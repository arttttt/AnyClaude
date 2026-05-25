//! Scroll prototype demo. Renders a column of coloured stripes; subsequent
//! commits add scroll input, velocity tracking, and the momentum integrator.

use std::sync::Arc;

use term_gpu::{GpuRenderer, RectInstance};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId};

const STRIPE_COUNT: usize = 1000;
const STRIPE_HEIGHT: f32 = 24.0;
const STRIPE_GAP: f32 = 2.0;
const STRIPE_X_MARGIN: f32 = 48.0;

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 4] {
    let h = h.rem_euclid(360.0);
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r1, g1, b1) = match h as u32 {
        0..=59 => (c, x, 0.0),
        60..=119 => (x, c, 0.0),
        120..=179 => (0.0, c, x),
        180..=239 => (0.0, x, c),
        240..=299 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r1 + m, g1 + m, b1 + m, 1.0]
}

fn build_stripes(window_width: f32) -> Vec<RectInstance> {
    let width = (window_width - STRIPE_X_MARGIN * 2.0).max(64.0);
    (0..STRIPE_COUNT)
        .map(|i| RectInstance {
            pos: [
                STRIPE_X_MARGIN,
                i as f32 * (STRIPE_HEIGHT + STRIPE_GAP),
            ],
            size: [width, STRIPE_HEIGHT],
            color: hsv_to_rgb((i as f32 * 1.7) % 360.0, 0.55, 0.92),
        })
        .collect()
}

#[derive(Default)]
struct App {
    window: Option<Arc<Window>>,
    renderer: Option<GpuRenderer>,
    stripes: Vec<RectInstance>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = WindowAttributes::default()
            .with_title("term_gpu scroll demo")
            .with_inner_size(winit::dpi::LogicalSize::new(960.0, 720.0));
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create window"),
        );

        let renderer = GpuRenderer::new(window.clone());
        self.stripes = build_stripes(renderer.size().width as f32);
        self.window = Some(window);
        self.renderer = Some(renderer);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(new_size) => {
                if let Some(r) = self.renderer.as_mut() {
                    r.resize(new_size);
                    self.stripes = build_stripes(new_size.width as f32);
                }
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                if let (Some(r), Some(w)) = (self.renderer.as_ref(), self.window.as_ref()) {
                    w.pre_present_notify();
                    r.render(&self.stripes, 0.0);
                }
            }
            _ => {}
        }
    }
}

fn main() {
    let event_loop = EventLoop::new().expect("failed to create event loop");
    let mut app = App::default();
    event_loop.run_app(&mut app).expect("event loop failed");
}
