//! Phase B proof vehicle (§15) — the retained+reactive COORDINATOR pattern,
//! proven on a chrome stub through a real `GpuRenderer` WITHOUT touching the
//! live `src/ui/gpu` app (migration rule: build alongside the old path, swap
//! `main.rs` last). The real anyclaude coordinator that replaces `GpuApp`
//! assembles in Phases C/D/E.
//!
//! - **B.1**: AppState + `view(&AppState)` + static render.
//! - **B.2 (this commit):** the two-phase reactive frame (R7). Raw input →
//!   `Msg` (event phase, no tree/AppState mutation) → `apply(&mut AppState,
//!   Msg)` (apply phase, the one mutation path, R6) → a single dirty signal →
//!   `reconcile_root` against the prior view (reconcile phase). The retained
//!   tree now PERSISTS across frames and is reconciled incrementally, not
//!   rebuilt. Type to see it; Backspace deletes; Esc clears.
//! - **B.3** adds the `next_wake` ticker + `frame_now` threading.
//!
//! Run: `cargo run -p term_ui --example coordinator`.

use std::sync::Arc;

use glam::Vec2;
use term_gpu::{FontFamily, FontSystem, GpuRenderer, RenderLayer, SwashCache, TextShapeCache};
use term_ui::{
    build_root, measure, paint, place, reconcile_root, NodeId, PaintOutput, RetainedTree,
    SizeConstraint, Stack, Text, WidgetId,
};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};

const FONT_SIZE: f32 = 22.0;
const FG: [f32; 4] = [0.90, 0.90, 0.95, 1.0];
const CYAN: [f32; 4] = [0.40, 0.85, 0.95, 1.0];
const DIM: [f32; 4] = [0.55, 0.55, 0.60, 1.0];

/// The SINGLE source of UI-decision truth (R2): plain data, no GPU handles, no
/// `Rc`/`RefCell`. `input` is the only mutable fact in B.2 and it changes ONLY
/// through `apply` (R6).
struct AppState {
    input: String,
}

impl AppState {
    fn new() -> Self {
        Self { input: String::new() }
    }
}

/// Intents — the ONLY vocabulary by which `AppState` changes (R6). Produced in
/// the event phase from raw input, consumed by `apply` in the apply phase.
/// Deliberately NOT applied during a tree borrow (R7).
enum Msg {
    Type(char),
    Backspace,
    Clear,
}

/// Apply phase: the single authoritative mutation (R6). Returns whether the
/// state actually changed — that boolean IS the dirty signal. Pure w.r.t. the
/// tree/resources; trivially unit-testable without a GPU (the Phase B+ pattern).
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
/// mutation here (R7) — it only translates input into `Msg`s.
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

/// Declarative view: a pure function of `&AppState` (R5/R6). Stable id-path
/// `WidgetId`s (R8). Only the middle line's text depends on `input`, so a
/// keystroke drives a single Text-node `reconcile` (the mutate path).
fn view(state: &AppState) -> Stack {
    Stack::vstack()
        .id(WidgetId::from_path(&[0]))
        .gap(8.0)
        .child(
            Text::new(
                "term_ui coordinator — Phase B.2  (type · Backspace · Esc clears)",
                FONT_SIZE,
                FG,
            )
            .id(WidgetId::from_path(&[0, 0])),
        )
        .child(
            Text::new(format!("> {}", state.input), FONT_SIZE, CYAN).id(WidgetId::from_path(&[0, 1])),
        )
        .child(
            Text::new(
                "event -> Msg -> apply -> dirty -> reconcile_root (incremental)",
                FONT_SIZE * 0.8,
                DIM,
            )
            .id(WidgetId::from_path(&[0, 2])),
        )
}

/// The coordinator: owns the resources + the one `AppState` + the PERSISTENT
/// retained tree (bucket-2 cache) + the last frame's view element (to diff
/// against) + the single dirty signal.
struct Coordinator {
    state: AppState,
    tree: RetainedTree,
    /// Last frame's view element — `reconcile_root` diffs the new view against
    /// it (the retained-mode incremental path). `None` until the first build.
    prev_view: Option<Stack>,
    root: Option<NodeId>,
    /// The single dirty signal (R6): set by `apply`, drained by `render`.
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
        Self {
            state: AppState::new(),
            tree: RetainedTree::new(),
            prev_view: None,
            root: None,
            dirty: true, // force the first build
            window: None,
            renderer: None,
            fonts: FontSystem::new(),
            swash: SwashCache::new(),
            shape: TextShapeCache::with_family(FontFamily::SansSerif),
            scratch: PaintOutput::default(),
            scale_factor: 1.0,
        }
    }

    /// Render one frame. The RECONCILE phase touches the tree only when dirty:
    /// first frame `build_root`; later dirty frames `reconcile_root` against the
    /// prior view (incremental, not a rebuild). Layout + paint run every frame
    /// (idempotent; also handles window resize without a reconcile).
    fn render(&mut self) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let sf = self.scale_factor;

        // ── Reconcile phase (R7): refresh the retained tree from AppState. ──
        if self.root.is_none() {
            let v = view(&self.state);
            let root = build_root(&mut self.tree, &v);
            self.root = Some(root);
            self.prev_view = Some(v);
        } else if self.dirty {
            let next = view(&self.state);
            let prev = self.prev_view.take().expect("prev_view present once built");
            reconcile_root(&mut self.tree, self.root.unwrap(), &prev, &next);
            self.prev_view = Some(next);
        }
        self.dirty = false;
        let root = self.root.unwrap();

        // ── Layout + paint (every frame; cheap, and absorbs window resize). ──
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
}

fn main() {
    let event_loop = EventLoop::new().expect("event loop");
    let mut coord = Coordinator::new();
    event_loop.run_app(&mut coord).expect("run_app");
}
