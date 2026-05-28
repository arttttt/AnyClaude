//! Phase B proof vehicle (§15) — the retained+reactive COORDINATOR pattern,
//! proven on a chrome stub through a real `GpuRenderer` WITHOUT touching the
//! live `src/ui/gpu` app (migration rule: build alongside the old path, swap
//! `main.rs` last). The real anyclaude coordinator that replaces `GpuApp`
//! assembles in Phases C/D/E.
//!
//! - **B.1**: AppState + `view(&AppState)` + static render.
//! - **B.2**: the two-phase reactive frame (R7) — input → `Msg` → `apply` →
//!   one dirty signal → incremental `reconcile_root`.
//! - **B.3 (this commit):** the `next_wake` ticker + `frame_now` threading.
//!   `view(&AppState, frame_now)` samples a fixed per-frame `Instant` (R4
//!   determinism); a live "uptime" line is DERIVED from it (R12, never stored).
//!   `next_wake` drives `ControlFlow::WaitUntil`, so the UI updates on a timer
//!   WITHOUT input — the same mechanism that will later drive caret blink and
//!   popup animations (their deadlines mined into `next_wake`).
//!
//! Run: `cargo run -p term_ui --example coordinator` (type to edit the input
//! line; the uptime line ticks on its own).

use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::Vec2;
use term_gpu::{FontFamily, FontSystem, GpuRenderer, RenderLayer, SwashCache, TextShapeCache};
use term_ui::{
    build_root, measure, paint, place, reconcile_root, NodeId, PaintOutput, RetainedTree,
    SizeConstraint, Stack, Text, WidgetId,
};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};

const FONT_SIZE: f32 = 22.0;
const FG: [f32; 4] = [0.90, 0.90, 0.95, 1.0];
const CYAN: [f32; 4] = [0.40, 0.85, 0.95, 1.0];
const AMBER: [f32; 4] = [0.95, 0.75, 0.35, 1.0];
const DIM: [f32; 4] = [0.55, 0.55, 0.60, 1.0];

/// Demo refresh cadence for the time-derived line (10 Hz).
const TICK: Duration = Duration::from_millis(100);

/// The SINGLE source of UI-decision truth (R2): plain data, no GPU handles, no
/// `Rc`/`RefCell`. `input` changes only through `apply` (R6). `started` is a
/// timer EPOCH (bucket 1, like the design's caret/animation epochs); the
/// elapsed "uptime" shown in the view is DERIVED from `frame_now - started`,
/// never stored (R12).
struct AppState {
    input: String,
    started: Instant,
}

impl AppState {
    fn new(now: Instant) -> Self {
        Self { input: String::new(), started: now }
    }
}

/// Intents — the ONLY vocabulary by which `AppState` changes (R6). Produced in
/// the event phase, consumed by `apply`. Never applied during a tree borrow (R7).
enum Msg {
    Type(char),
    Backspace,
    Clear,
}

/// Apply phase: the single authoritative mutation (R6). Returns whether the
/// state actually changed — that boolean feeds the one dirty signal. Pure
/// w.r.t. the tree/resources; unit-testable without a GPU (the Phase C+ pattern).
fn apply(state: &mut AppState, msg: Msg) -> bool {
    match msg {
        Msg::Type(c) if !c.is_control() => {
            state.input.push(c);
            true
        }
        Msg::Type(_) => false,
        Msg::Backspace => state.input.pop().is_some(),
        Msg::Clear => {
            let changed = !state.input.is_empty();
            state.input.clear();
            changed
        }
    }
}

/// Event phase: map a raw key event to intents. No tree borrow, no AppState
/// mutation (R7) — pure translation of input into `Msg`s.
fn key_to_msgs(ev: &KeyEvent) -> Vec<Msg> {
    if ev.state != ElementState::Pressed {
        return Vec::new();
    }
    match &ev.logical_key {
        Key::Named(NamedKey::Backspace) => vec![Msg::Backspace],
        Key::Named(NamedKey::Escape) => vec![Msg::Clear],
        Key::Named(NamedKey::Space) => vec![Msg::Type(' ')],
        Key::Character(s) => s.chars().map(Msg::Type).collect(),
        _ => Vec::new(),
    }
}

/// When should the loop next wake to repaint? The min of all active deadlines.
/// Demo: a live uptime line wants ~10 Hz, so the next deadline is `now + TICK`.
/// The real coordinator mins over animation / caret-blink / momentum deadlines
/// and returns `None` when fully idle (→ `ControlFlow::Wait`, no repaints).
fn next_wake(_state: &AppState, now: Instant) -> Option<Instant> {
    Some(now + TICK)
}

/// Declarative view: a pure function of `(&AppState, frame_now)` (R5/R6). The
/// input line is AppState-driven; the uptime line is `frame_now`-derived (R12).
/// Stable id-path `WidgetId`s (R8).
fn view(state: &AppState, frame_now: Instant) -> Stack {
    let uptime = (frame_now - state.started).as_secs_f32();
    Stack::vstack()
        .id(WidgetId::from_path(&[0]))
        .gap(8.0)
        .child(
            Text::new(
                "term_ui coordinator — Phase B.3  (type · Backspace · Esc clears)",
                FONT_SIZE,
                FG,
            )
            .id(WidgetId::from_path(&[0, 0])),
        )
        .child(
            Text::new(format!("> {}", state.input), FONT_SIZE, CYAN).id(WidgetId::from_path(&[0, 1])),
        )
        .child(
            Text::new(format!("uptime {uptime:.1}s  (ticked by next_wake, no input)"), FONT_SIZE, AMBER)
                .id(WidgetId::from_path(&[0, 2])),
        )
        .child(
            Text::new(
                "event/timer -> dirty -> reconcile_root (incremental) -> measure/place/paint",
                FONT_SIZE * 0.8,
                DIM,
            )
            .id(WidgetId::from_path(&[0, 3])),
        )
}

/// The coordinator: resources + the one `AppState` + the PERSISTENT retained
/// tree + the prior view (to diff) + the single dirty signal.
struct Coordinator {
    state: AppState,
    tree: RetainedTree,
    prev_view: Option<Stack>,
    root: Option<NodeId>,
    /// The single dirty signal (R6): set by `apply` (input) AND by the timer
    /// tick (frame_now advanced ⇒ the time-derived view differs); drained by
    /// `render`.
    dirty: bool,
    window: Option<Arc<Window>>,
    renderer: Option<GpuRenderer>,
    fonts: FontSystem,
    swash: SwashCache,
    shape: TextShapeCache,
    scratch: PaintOutput,
    scale_factor: f32,
}

impl Coordinator {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            state: AppState::new(now),
            tree: RetainedTree::new(),
            prev_view: None,
            root: None,
            dirty: true,
            window: None,
            renderer: None,
            fonts: FontSystem::new(),
            swash: SwashCache::new(),
            shape: TextShapeCache::with_family(FontFamily::SansSerif),
            scratch: PaintOutput::default(),
            scale_factor: 1.0,
        }
    }

    fn render(&mut self) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let sf = self.scale_factor;
        let frame_now = Instant::now(); // fixed for this frame (R4 determinism)

        // ── Reconcile phase (R7): refresh the retained tree from (AppState, ──
        //    frame_now), only when dirty. First frame builds; later dirty
        //    frames reconcile incrementally against the prior view (R5).
        if self.root.is_none() {
            let v = view(&self.state, frame_now);
            let root = build_root(&mut self.tree, &v);
            self.root = Some(root);
            self.prev_view = Some(v);
        } else if self.dirty {
            let next = view(&self.state, frame_now);
            let prev = self.prev_view.take().expect("prev_view present once built");
            reconcile_root(&mut self.tree, self.root.unwrap(), &prev, &next);
            self.prev_view = Some(next);
        }
        self.dirty = false;
        let root = self.root.unwrap();

        // ── Layout + paint (every frame; absorbs window resize). ──
        let logical = Vec2::new(
            renderer.size().width as f32 / sf,
            renderer.size().height as f32 / sf,
        );
        measure(
            &mut self.tree,
            root,
            SizeConstraint::loose(logical),
            &mut self.fonts,
            &mut self.shape,
            sf,
        );
        place(&mut self.tree, root, Vec2::new(16.0, 16.0));

        self.scratch.clear();
        paint(
            &self.tree,
            root,
            &mut self.scratch,
            renderer.atlas_mut(),
            &mut self.fonts,
            &mut self.swash,
            &mut self.shape,
            sf,
        );

        window.pre_present_notify();
        renderer.render(
            RenderLayer::rects_and_glyphs(&self.scratch.rects, &self.scratch.glyphs),
            None,
            0.0,
        );
        self.shape.end_frame();
    }
}

impl ApplicationHandler for Coordinator {
    /// The timer fired (a `WaitUntil` deadline elapsed): mark dirty + repaint.
    /// This is the time-driven half of the single dirty signal (R6).
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
        if matches!(cause, StartCause::ResumeTimeReached { .. }) {
            self.dirty = true;
            if let Some(w) = self.window.as_ref() {
                w.request_redraw();
            }
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = WindowAttributes::default()
            .with_title("term_ui coordinator")
            .with_inner_size(winit::dpi::LogicalSize::new(720.0, 420.0));
        let window = Arc::new(event_loop.create_window(attrs).expect("create_window"));
        let renderer = GpuRenderer::new(window.clone());
        self.scale_factor = renderer.scale_factor();
        self.window = Some(window);
        self.renderer = Some(renderer);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(r) = self.renderer.as_mut() {
                    r.resize(size);
                }
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale_factor = scale_factor as f32;
                if let Some(r) = self.renderer.as_mut() {
                    r.set_scale_factor(self.scale_factor);
                }
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            // ── Event phase (R7): raw input → Msg → apply → dirty signal. ──
            WindowEvent::KeyboardInput { event, .. } => {
                let mut changed = false;
                for msg in key_to_msgs(&event) {
                    changed |= apply(&mut self.state, msg);
                }
                if changed {
                    self.dirty = true;
                    if let Some(w) = self.window.as_ref() {
                        w.request_redraw();
                    }
                }
            }
            WindowEvent::RedrawRequested => self.render(),
            _ => {}
        }
    }

    /// Schedule the next wake from `next_wake` (the ticker). `WaitUntil` makes
    /// the loop sleep until the deadline, then fire `new_events`
    /// (`ResumeTimeReached`); `Wait` (when idle) sleeps until the next input.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        match next_wake(&self.state, Instant::now()) {
            Some(wake) => event_loop.set_control_flow(ControlFlow::WaitUntil(wake)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }
}

fn main() {
    let event_loop = EventLoop::new().expect("event loop");
    let mut coord = Coordinator::new();
    event_loop.run_app(&mut coord).expect("run_app");
}
