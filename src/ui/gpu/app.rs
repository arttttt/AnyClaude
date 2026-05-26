//! Winit `ApplicationHandler` skeleton for the GPU UI.
//!
//! This file currently only opens a window and draws an empty
//! background. Subsequent commits in Phase 5 layer functionality on
//! top: PTY rendering (C2), input (C3), header/footer chrome (C4-C5),
//! drop-shadow shader and popup overlays (C6-C9). The `--gpu` CLI flag
//! routes here for incremental verification; it is removed in the C10
//! cutover commit.

use std::sync::Arc;

use term_gpu::GpuRenderer;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId};

const INITIAL_W: f32 = 1200.0;
const INITIAL_H: f32 = 800.0;

/// Entry point for the GPU UI. Signature mirrors `ui::run` so the
/// C10 cutover only has to flip which function `main.rs` calls.
///
/// `_backend_override` and `_claude_args` are accepted but ignored at
/// this stage — PTY wiring lands in C2.
pub fn run(_backend_override: Option<String>, _claude_args: Vec<String>) -> std::io::Result<()> {
    let event_loop = EventLoop::new().map_err(|e| std::io::Error::other(e.to_string()))?;
    let mut app = GpuApp::new();
    event_loop
        .run_app(&mut app)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(())
}

struct GpuApp {
    window: Option<Arc<Window>>,
    renderer: Option<GpuRenderer>,
    scale_factor: f32,
}

impl GpuApp {
    fn new() -> Self {
        Self {
            window: None,
            renderer: None,
            scale_factor: 1.0,
        }
    }
}

impl ApplicationHandler for GpuApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = WindowAttributes::default()
            .with_title("anyclaude")
            .with_inner_size(LogicalSize::new(INITIAL_W, INITIAL_H));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("anyclaude: failed to create window: {e}");
                event_loop.exit();
                return;
            }
        };
        let renderer = GpuRenderer::new(window.clone());
        self.scale_factor = renderer.scale_factor();
        self.window = Some(window.clone());
        self.renderer = Some(renderer);
        window.request_redraw();
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
            WindowEvent::RedrawRequested => {
                if let Some(r) = self.renderer.as_mut() {
                    r.render(&[], &[], 0.0);
                }
            }
            _ => {}
        }
    }
}
