//! Scroll prototype demo. Renders a column of coloured stripes; subsequent
//! commits add scroll input, velocity tracking, and the momentum integrator.

use std::sync::Arc;

use term_gpu::{GpuRenderer, RectInstance, ScrollState, NUM_PIXELS_PER_LINE};
use winit::application::ApplicationHandler;
use winit::event::{MouseScrollDelta, WindowEvent};
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

fn stripes_total_height() -> f32 {
    STRIPE_COUNT as f32 * (STRIPE_HEIGHT + STRIPE_GAP) - STRIPE_GAP
}

#[derive(Default)]
struct App {
    window: Option<Arc<Window>>,
    renderer: Option<GpuRenderer>,
    stripes: Vec<RectInstance>,
    scroll: ScrollState,
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
        self.scroll = ScrollState {
            offset_y: 0.0,
            total_size_px: stripes_total_height(),
            visible_px: renderer.size().height as f32,
        };
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
                    self.scroll.visible_px = new_size.height as f32;
                    self.scroll.scroll_by(0.0); // re-clamp against new max
                }
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::PixelDelta(p) => p.y as f32,
                    MouseScrollDelta::LineDelta(_, v) => v * NUM_PIXELS_PER_LINE,
                };
                // winit reports positive y for "swipe up" / "wheel away".
                // We treat swipe-up as "show content below" → offset grows.
                self.scroll.scroll_by(-dy);
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                if let (Some(r), Some(w)) = (self.renderer.as_ref(), self.window.as_ref()) {
                    w.pre_present_notify();
                    r.render(&self.stripes, self.scroll.offset_y);
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
