//! Phase B proof vehicle (§15) — the retained+reactive COORDINATOR pattern,
//! proven on a chrome stub through a real `GpuRenderer` WITHOUT touching the
//! live `src/ui/gpu` app (migration rule: build alongside the old path, swap
//! `main.rs` last). The real anyclaude coordinator that replaces `GpuApp`
//! assembles in Phases C/D/E.
//!
//! **B.1 (this commit):** a single plain `AppState` (R2, the only source of
//! UI-decision truth), a declarative `view(&AppState)` (R5/R6), and a STATIC
//! render (`build_root → measure → place → paint → render`). The two-phase
//! frame (`event → Msg → apply → reconcile`, R7) lands in B.2; the `next_wake`
//! ticker + `frame_now` threading in B.3.
//!
//! Run: `cargo run -p term_ui --example coordinator` (draws a header + body
//! stub derived entirely from `AppState`).

use std::sync::Arc;

use glam::Vec2;
use term_gpu::{FontFamily, FontSystem, GpuRenderer, RenderLayer, SwashCache, TextShapeCache};
use term_ui::{
    build_root, measure, paint, place, PaintOutput, RetainedTree, SizeConstraint, Stack, Text,
    WidgetId,
};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId};

const FONT_SIZE: f32 = 22.0;
const FG: [f32; 4] = [0.90, 0.90, 0.95, 1.0];
const DIM: [f32; 4] = [0.55, 0.55, 0.60, 1.0];

/// The SINGLE source of UI-decision truth (R2): plain data, no GPU handles, no
/// `Rc`/`RefCell`. Static in B.1; B.2 mutates it through `apply(&mut AppState,
/// Msg)` and B.3 advances a `frame_now`-derived counter via the ticker.
struct AppState {
    header: String,
    body: Vec<String>,
}

impl AppState {
    fn new() -> Self {
        Self {
            header: "term_ui coordinator — Phase B.1 (static render)".into(),
            body: vec![
                "AppState is the single source of truth (R2)".into(),
                "view(&AppState) -> retained tree -> measure/place/paint".into(),
                "events (B.2) and the next_wake ticker (B.3) come next".into(),
            ],
        }
    }
}

/// Declarative view: a pure function of `&AppState` (R5/R6). A `VStack` chrome
/// stub — a header line plus the body lines. Each node carries a stable id-path
/// `WidgetId` (R8) so identity derives from tree position-by-key, not a slot.
fn view(state: &AppState) -> Stack {
    let mut col = Stack::vstack()
        .id(WidgetId::from_path(&[0]))
        .gap(8.0)
        .child(Text::new(state.header.clone(), FONT_SIZE, FG).id(WidgetId::from_path(&[0, 0])));
    for (i, line) in state.body.iter().enumerate() {
        col = col.child(
            Text::new(line.clone(), FONT_SIZE * 0.8, DIM).id(WidgetId::from_path(&[0, 1, i as u64])),
        );
    }
    col
}

/// The coordinator: owns the resources + the one `AppState` + the retained tree
/// (bucket-2 cache). B.1 holds no `Msg`/`apply` yet — those arrive in B.2 with
/// their consumer (event routing), per R13 (no abstraction before its consumer).
struct Coordinator {
    state: AppState,
    tree: RetainedTree,
    window: Option<Arc<Window>>,
    renderer: Option<GpuRenderer>,
    fonts: FontSystem,
    swash: SwashCache,
    shape: TextShapeCache,
    /// Reused scratch buffers across frames (§14) — never reallocated per frame.
    scratch: PaintOutput,
    scale_factor: f32,
}

impl Coordinator {
    fn new() -> Self {
        Self {
            state: AppState::new(),
            tree: RetainedTree::new(),
            window: None,
            renderer: None,
            fonts: FontSystem::new(),
            swash: SwashCache::new(),
            shape: TextShapeCache::with_family(FontFamily::SansSerif),
            scratch: PaintOutput::default(),
            scale_factor: 1.0,
        }
    }

    /// Render one frame from `AppState`. B.1 rebuilds the tree each frame
    /// (`build_root`); B.2 switches to incremental `reconcile_root` against the
    /// prior frame's view, driven by the single dirty signal.
    fn render(&mut self) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let sf = self.scale_factor;

        self.tree = RetainedTree::new();
        let root = build_root(&mut self.tree, &view(&self.state));

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
