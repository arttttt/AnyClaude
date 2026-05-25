//! Scroll prototype demo. Opens a window via winit; subsequent commits add
//! the wgpu pipeline, scroll input, velocity tracking, and momentum integrator.

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId};

#[derive(Default)]
struct App {
    window: Option<Window>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = WindowAttributes::default()
            .with_title("term_gpu scroll demo")
            .with_inner_size(winit::dpi::LogicalSize::new(960.0, 720.0));
        let window = event_loop
            .create_window(attrs)
            .expect("failed to create window");
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => {
                if let Some(w) = self.window.as_ref() {
                    w.pre_present_notify();
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
