//! Mini-integration of `term_core` × `term_gpu`.
//!
//! Pipes raw ANSI bytes from stdin through `term_core::VtEmulator` into a
//! GPU window rendered by `term_gpu::GpuRenderer`. This commit lands only
//! the plumbing — stdin reader thread, emulator wired to a redraw signal,
//! and an empty render pass. Cell-to-glyph translation, background rects,
//! and the cursor land in subsequent commits.
//!
//! ## Run
//!
//! ```bash
//! cat session.log | cargo run -p term_gpu --example render_term --release
//! ```
//!
//! The emulator's PTY responses (DA, DSR) are taken and discarded here —
//! there is no PTY to write back to in this demo.
//!
//! ## Threading
//!
//! winit's event loop owns the main thread on macOS. A separate reader
//! thread reads stdin into 4 KiB chunks, ships them across an `mpsc`
//! channel, and signals the event loop via `EventLoopProxy::send_event`.
//! The signal is the redraw trigger, the channel carries the bytes — kept
//! separate so a backed-up event loop never blocks the reader.

use std::io::Read;
use std::sync::mpsc;
use std::sync::Arc;

use term_core::{create_emulator, TerminalEmulator};
use term_gpu::{GlyphInstance, GpuRenderer, RectInstance};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowAttributes, WindowId};

const DEFAULT_COLS: usize = 80;
const DEFAULT_ROWS: usize = 24;
const DEFAULT_SCROLLBACK: usize = 1000;

/// Custom event signalling that the stdin reader has shipped at least one
/// chunk into the channel. The handler drains the channel and requests a
/// redraw.
#[derive(Debug, Clone, Copy)]
enum CustomEvent {
    BytesArrived,
}

struct App {
    window: Option<Arc<Window>>,
    renderer: Option<GpuRenderer>,
    emulator: Box<dyn TerminalEmulator>,
    bytes_rx: mpsc::Receiver<Vec<u8>>,
}

impl App {
    fn new(bytes_rx: mpsc::Receiver<Vec<u8>>) -> Self {
        Self {
            window: None,
            renderer: None,
            emulator: create_emulator(DEFAULT_COLS, DEFAULT_ROWS, DEFAULT_SCROLLBACK),
            bytes_rx,
        }
    }

    /// Drain every queued chunk into the emulator. Responses are discarded
    /// — this demo replays a recorded byte stream, there is no PTY to
    /// answer DA/DSR queries on.
    fn drain_bytes(&mut self) {
        while let Ok(chunk) = self.bytes_rx.try_recv() {
            self.emulator.process(&chunk);
        }
        let _ = self.emulator.take_responses();
    }

    fn on_redraw(&mut self) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let Some(window) = self.window.as_ref() else {
            return;
        };

        // V1: empty buffers. The renderer's clear colour shows through.
        // Cell translation lands in commit 2.
        let rects: Vec<RectInstance> = Vec::new();
        let glyphs: Vec<GlyphInstance> = Vec::new();

        window.pre_present_notify();
        renderer.render(&rects, &glyphs, 0.0);
    }
}

impl ApplicationHandler<CustomEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = WindowAttributes::default()
            .with_title("term_gpu \u{00d7} term_core")
            .with_inner_size(winit::dpi::LogicalSize::new(960.0, 600.0));
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create window"),
        );
        let renderer = GpuRenderer::new(window.clone());
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
            WindowEvent::RedrawRequested => self.on_redraw(),
            _ => {}
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: CustomEvent) {
        match event {
            CustomEvent::BytesArrived => {
                self.drain_bytes();
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
        }
    }
}

/// Spawn a reader thread that pumps stdin into `tx` in 4 KiB chunks and
/// signals the event loop after every chunk. Exits on EOF or if the event
/// loop has dropped its receiver.
fn spawn_stdin_reader(tx: mpsc::Sender<Vec<u8>>, proxy: EventLoopProxy<CustomEvent>) {
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 4096];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                    if proxy.send_event(CustomEvent::BytesArrived).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
}

fn main() {
    let event_loop = EventLoop::<CustomEvent>::with_user_event()
        .build()
        .expect("failed to create event loop");
    let proxy = event_loop.create_proxy();
    let (bytes_tx, bytes_rx) = mpsc::channel();
    spawn_stdin_reader(bytes_tx, proxy);
    let mut app = App::new(bytes_rx);
    event_loop.run_app(&mut app).expect("event loop failed");
}
