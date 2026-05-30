//! Phase A proof vehicle (§15): drive the term_ui engine end-to-end through a
//! real `GpuRenderer`. Authors a static `VStack(Text, HStack(Text, Text))`,
//! reconciles it once (proving the incremental path runs, not just `build`),
//! then runs the full `measure → place → paint` pipeline and submits the
//! resulting `RectInstance`/`GlyphInstance` stream to the GPU.
//!
//! This is compiled headlessly under `cargo build --examples`; *running* it
//! opens a window and is manual validation (it draws three labels in a column).
//! The behavioral gate is the headless `tests/r4_reconcile_identity.rs`.

use std::sync::Arc;

use glam::Vec2;
use term_gpu::{
    FontFamily, FontSystem, GpuRenderer, RenderLayer, SwashCache, TextShapeCache,
};
use term_ui::{
    build_root, measure, paint, place, reconcile_root, BoxView, PaintOutput, RetainedTree,
    SizeConstraint, Stack, Text, WidgetId,
};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId};

const FONT_SIZE: f32 = 28.0;
const WHITE: [f32; 4] = [0.92, 0.92, 0.95, 1.0];
const CYAN: [f32; 4] = [0.4, 0.85, 0.95, 1.0];
const AMBER: [f32; 4] = [0.95, 0.75, 0.35, 1.0];

/// The Phase A static view: `VStack(Text, HStack(Text, Text))`. Each label is
/// given a stable id-path `WidgetId` (R8) so identity is derived from tree
/// position-by-key, not from an arena slot.
fn root_view() -> Stack {
    let row = Stack::hstack()
        .id(WidgetId::from_path(&[0, 1]))
        .gap(16.0)
        .child(Text::new("left", FONT_SIZE, CYAN).id(WidgetId::from_path(&[0, 1, 0])))
        .child(Text::new("right", FONT_SIZE, AMBER).id(WidgetId::from_path(&[0, 1, 1])));

    Stack::vstack()
        .id(WidgetId::from_path(&[0]))
        .gap(12.0)
        .child(Text::new("term_ui toy", FONT_SIZE, WHITE).id(WidgetId::from_path(&[0, 0])))
        .child_boxed(Box::new(row) as BoxView, term_ui::Sizing::Auto)
}

struct App {
    window: Option<Arc<Window>>,
    renderer: Option<GpuRenderer>,
    fonts: FontSystem,
    swash: SwashCache,
    shape: TextShapeCache,
    scale_factor: f32,
}

impl App {
    fn new() -> Self {
        Self {
            window: None,
            renderer: None,
            fonts: FontSystem::new(),
            swash: SwashCache::new(),
            shape: TextShapeCache::with_family(FontFamily::SansSerif),
            scale_factor: 1.0,
        }
    }

    fn on_redraw(&mut self) {
        let Self {
            renderer,
            window,
            fonts,
            swash,
            shape,
            scale_factor,
        } = self;
        let Some(renderer) = renderer.as_mut() else {
            return;
        };
        let Some(window) = window.as_ref() else {
            return;
        };
        let sf = *scale_factor;

        // Build the retained tree, then reconcile an identical view into it so
        // the example exercises the incremental path (`reconcile_root`), not
        // only `build_root`.
        let mut tree = RetainedTree::new();
        let root = build_root(&mut tree, &root_view());
        let next = root_view();
        let prev = root_view();
        reconcile_root(&mut tree, root, &prev, &next);

        // measure → place against the window's logical size.
        let logical = Vec2::new(
            renderer.size().width as f32 / sf,
            renderer.size().height as f32 / sf,
        );
        measure(
            &mut tree,
            root,
            SizeConstraint::loose(logical),
            fonts,
            shape,
            sf,
        );
        place(&mut tree, root, Vec2::new(24.0, 24.0));

        // paint into caller-owned buffers, then submit.
        let mut out = PaintOutput::default();
        paint(
            &tree,
            root,
            &mut out,
            renderer.atlas_mut(),
            fonts,
            swash,
            shape,
            sf,
        );

        window.pre_present_notify();
        renderer.render(RenderLayer::rects_and_glyphs(&out.rects, &out.glyphs), None, 0.0);
        shape.end_frame();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = WindowAttributes::default()
            .with_title("term_ui toy")
            .with_inner_size(winit::dpi::LogicalSize::new(640.0, 360.0));
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create window"),
        );
        let renderer = GpuRenderer::new(window.clone());
        self.scale_factor = renderer.scale_factor();
        self.window = Some(window);
        self.renderer = Some(renderer);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(new_size) => {
                if let Some(r) = self.renderer.as_mut() {
                    r.resize(new_size);
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
            WindowEvent::RedrawRequested => self.on_redraw(),
            _ => {}
        }
    }
}

fn main() {
    let event_loop = EventLoop::new().expect("failed to create event loop");
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("event loop failed");
}
