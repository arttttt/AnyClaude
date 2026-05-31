//! Phase C visual-verification vehicle: the REAL anyclaude header/footer chrome,
//! rendered as declarative term_ui views (uikit bars fed by the
//! `ui::chrome_labels` presenter) through a real `GpuRenderer` — WITHOUT
//! touching the live `ui::gpu` app. Mirrors the Phase B coordinator pattern
//! (AppState + Msg/apply + `view(&AppState, frame_now)` + an absolute-deadline
//! ticker); this is the rehearsal for the coordinator that replaces `GpuApp` in
//! Phase E.
//!
//! It lives as an anyclaude example (not a term_ui example) because the chrome
//! text is DOMAIN data (backend / session / Reqs / uptime), and term_ui is a
//! lower crate that cannot see it. The example feeds the bars sample data.
//!
//! Run: `cargo run --example chrome_preview`
//!   · type 'r' → Reqs + 1
//!   · type 'c' → trigger the "Session ID copied!" flash (auto-expires ~1.5s)
//!   · the Uptime line ticks on its own (no input), and KEEPS ticking while a
//!     key is held (the absolute-deadline ticker, see Phase B's lesson).

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyclaude::ui::chrome_labels::{
    footer_segments, header_segments, CHROME_SEPARATOR_COLOR, CHROME_TEXT_COLOR, HEADER_SEPARATOR,
};
use glam::Vec2;
use term_gpu::{FontFamily, FontSystem, GpuRenderer, RenderLayer, SwashCache, TextShapeCache};
use term_ui::{
    build_root, measure, paint, place, reconcile_root, CrossAxis, Insets, Modified, Modifier,
    Modify, NodeId, PaintOutput, RetainedTree, SizeConstraint, Sizing, Stack, Text,
};
use uikit::{footer_bar, header_bar};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::Key;
use winit::window::{Window, WindowAttributes, WindowId};

/// Chrome text size (logical px) — matches the live `ui::gpu::chrome`.
const CHROME_FONT: f32 = 12.0;
/// Reserved strip heights — matches `HEADER_HEIGHT_LOGICAL`/`FOOTER_HEIGHT_LOGICAL`.
const HEADER_H: f32 = 24.0;
const FOOTER_H: f32 = 22.0;
/// Left inset of the header segment row (the live header starts text at x=8).
const LEADING_PAD: f32 = 8.0;
/// Demo ticker cadence (drives the Uptime line + flash expiry).
const TICK: Duration = Duration::from_millis(250);
/// How long the "Session ID copied!" flash stays up — matches the live chrome.
const SESSION_COPY_FLASH: Duration = Duration::from_millis(1500);
/// The binary's own version (a uikit-side `env!` would read the wrong crate).
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Panel fill for the placeholder content region (cosmetic — the chrome is
/// what's under test).
const PANEL_BG: [f32; 4] = [0.06, 0.06, 0.08, 1.0];
const PANEL_TEXT: [f32; 4] = [0.45, 0.45, 0.50, 1.0];

/// Sample UI truth (R2). `started` / `copied_at` are timer EPOCHS (bucket 1);
/// the Uptime value and the copied-flash boolean are DERIVED from `frame_now`
/// in `view` (R12), never stored as resolved booleans/strings.
struct AppState {
    backend: String,
    subagent: Option<String>,
    teammate: Option<String>,
    reqs: u64,
    session_id: String,
    started: Instant,
    copied_at: Option<Instant>,
}

impl AppState {
    fn sample(now: Instant) -> Self {
        Self {
            backend: "anthropic".to_string(),
            subagent: Some("opus".to_string()),
            teammate: None,
            reqs: 0,
            session_id: "a1b2c3d4-e5f6".to_string(),
            started: now,
            copied_at: None,
        }
    }
}

/// Intents (R6) — the only vocabulary by which `AppState` changes.
enum Msg {
    Req,
    CopyFlash,
}

/// Apply phase (R6): the single authoritative mutation. `now` stamps the flash
/// epoch (bucket 1). Returns whether anything changed (feeds the dirty signal).
fn apply(state: &mut AppState, msg: Msg, now: Instant) -> bool {
    match msg {
        Msg::Req => {
            state.reqs += 1;
            true
        }
        Msg::CopyFlash => {
            state.copied_at = Some(now);
            true
        }
    }
}

/// Event phase (R7): raw key → intents. No tree borrow, no mutation.
fn key_to_msgs(ev: &KeyEvent) -> Vec<Msg> {
    if ev.state != ElementState::Pressed {
        return Vec::new();
    }
    match &ev.logical_key {
        Key::Character(s) if s.as_str() == "r" => vec![Msg::Req],
        Key::Character(s) if s.as_str() == "c" => vec![Msg::CopyFlash],
        _ => Vec::new(),
    }
}

/// Declarative chrome view: header bar (Fixed 24) · panel (Fill) · footer bar
/// (Fixed 22), full-width (`CrossAxis::Stretch`). Uptime and the copied-flash
/// are derived from `frame_now` (R12); everything else from `AppState` (R5/R6).
fn view(state: &AppState, frame_now: Instant) -> Stack {
    let uptime = frame_now.duration_since(state.started).as_secs();
    let copied = state
        .copied_at
        .is_some_and(|t| frame_now.duration_since(t) < SESSION_COPY_FLASH);

    let header = header_segments(
        &state.backend,
        state.subagent.as_deref(),
        state.teammate.as_deref(),
        state.reqs,
        uptime,
        &state.session_id,
        copied,
    );
    let (left, right) = footer_segments(VERSION);

    Stack::vstack()
        .cross(CrossAxis::Stretch)
        .child_sized(
            header_bar(
                &header,
                HEADER_SEPARATOR,
                CHROME_TEXT_COLOR,
                CHROME_FONT,
                LEADING_PAD,
                CHROME_SEPARATOR_COLOR,
            ),
            Sizing::Fixed(HEADER_H),
        )
        .child_sized(panel(), Sizing::Fill)
        .child_sized(
            footer_bar(&left, &right, CHROME_FONT, CHROME_SEPARATOR_COLOR),
            Sizing::Fixed(FOOTER_H),
        )
}

/// Cosmetic content region between the chrome strips.
fn panel() -> Modified {
    Text::new(
        "terminal panel — chrome preview · 'r' Reqs+1 · 'c' Session-copied flash",
        CHROME_FONT,
        PANEL_TEXT,
    )
    .modify(Modifier::new().background(PANEL_BG).padding(Insets::all(12.0)))
}

/// The coordinator: resources + the one `AppState` + the persistent retained
/// tree + the prior view + the single dirty signal + the ticker's absolute wake.
struct Coordinator {
    state: AppState,
    tree: RetainedTree,
    prev_view: Option<Stack>,
    root: Option<NodeId>,
    dirty: bool,
    next_tick: Instant,
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
            state: AppState::sample(now),
            tree: RetainedTree::new(),
            prev_view: None,
            root: None,
            dirty: true,
            next_tick: now + TICK,
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

        // Reconcile only when dirty; first frame builds, later frames diff.
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

        // Layout to the FULL window (chrome flush to the edges) + paint.
        let logical = Vec2::new(
            renderer.size().width as f32 / sf,
            renderer.size().height as f32 / sf,
        );
        measure(
            &mut self.tree,
            root,
            SizeConstraint::tight(logical),
            &mut self.fonts,
            &mut self.shape,
            sf,
        );
        place(&mut self.tree, root, Vec2::ZERO);

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
            .with_title("anyclaude — chrome preview (Phase C)")
            .with_inner_size(winit::dpi::LogicalSize::new(900.0, 360.0));
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
            WindowEvent::KeyboardInput { event, .. } => {
                let now = Instant::now();
                let mut changed = false;
                for msg in key_to_msgs(&event) {
                    changed |= apply(&mut self.state, msg, now);
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

    /// Drive the ticker on an ABSOLUTE deadline polled after every event batch,
    /// so a due tick fires even when key-repeat churn starves
    /// `ResumeTimeReached` (the Phase B ticker lesson).
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if now >= self.next_tick {
            self.dirty = true;
            if let Some(w) = self.window.as_ref() {
                w.request_redraw();
            }
            while self.next_tick <= now {
                self.next_tick += TICK;
            }
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_tick));
    }
}

fn main() {
    let event_loop = EventLoop::new().expect("event loop");
    let mut coord = Coordinator::new();
    event_loop.run_app(&mut coord).expect("run_app");
}
