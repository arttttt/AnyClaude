//! Scroll prototype demo with pixel-based scroll + momentum integrator.
//!
//! Architecture follows `docs/design/gpu-terminal-scroll.md`:
//! - winit `MouseWheel` events feed `ScrollState` and `ScrollVelocity`
//! - after `GESTURE_END_TIMEOUT` of silence, a momentum timer kicks in
//! - the timer ticks every `MOMENTUM_FRAME_INTERVAL` and decays velocity
//! - both timers are abortable; a new wheel event cancels them

use std::sync::Arc;
use std::time::Instant;

use futures::future::{abortable, AbortHandle};
use futures_timer::Delay;
use glam::Vec2;
use term_gpu::{
    decay_velocity, GpuRenderer, RectInstance, ScrollState, ScrollVelocity, GESTURE_END_TIMEOUT,
    MOMENTUM_FRAME_INTERVAL, MOMENTUM_MIN_VELOCITY, MOMENTUM_THRESHOLD, NUM_PIXELS_PER_LINE,
};
use winit::application::ApplicationHandler;
use winit::event::{MouseScrollDelta, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowAttributes, WindowId};

const STRIPE_COUNT: usize = 1000;
const STRIPE_HEIGHT: f32 = 24.0;
const STRIPE_GAP: f32 = 2.0;
const STRIPE_X_MARGIN: f32 = 48.0;

// Ruler overlay parameters. Thin horizontal lines tick along the scroll
// space; their sub-pixel motion during inertia is the visible proof that
// scroll_offset_y is `f32`, not an integer line count.
const RULER_X: f32 = 8.0;
const RULER_SMALL_WIDTH: f32 = 16.0;
const RULER_BIG_WIDTH: f32 = 32.0;
const RULER_TICK_INTERVAL: f32 = 10.0;
const RULER_BIG_EVERY: usize = 10; // every 10 small ticks → 100 px

#[derive(Debug, Clone, Copy)]
enum CustomEvent {
    GestureEnded,
    MomentumTick,
}

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

fn stripes_total_height() -> f32 {
    STRIPE_COUNT as f32 * (STRIPE_HEIGHT + STRIPE_GAP) - STRIPE_GAP
}

fn build_stripes(window_width: f32) -> Vec<RectInstance> {
    let width = (window_width - STRIPE_X_MARGIN * 2.0).max(64.0);
    (0..STRIPE_COUNT)
        .map(|i| RectInstance {
            pos: [STRIPE_X_MARGIN, i as f32 * (STRIPE_HEIGHT + STRIPE_GAP)],
            size: [width, STRIPE_HEIGHT],
            color: hsv_to_rgb((i as f32 * 1.7) % 360.0, 0.55, 0.92),
        })
        .collect()
}

fn build_ruler(total_height: f32) -> Vec<RectInstance> {
    let count = (total_height / RULER_TICK_INTERVAL) as usize;
    (0..count)
        .map(|i| {
            let big = i % RULER_BIG_EVERY == 0;
            RectInstance {
                pos: [RULER_X, i as f32 * RULER_TICK_INTERVAL],
                size: [
                    if big { RULER_BIG_WIDTH } else { RULER_SMALL_WIDTH },
                    1.0,
                ],
                color: if big {
                    [1.0, 1.0, 1.0, 0.9]
                } else {
                    [1.0, 0.95, 0.45, 0.6]
                },
            }
        })
        .collect()
}

/// Spawn a one-shot abortable timer that sends `event` to the event loop
/// after `delay`. Returns the `AbortHandle` so the caller can cancel.
fn schedule_once(
    proxy: EventLoopProxy<CustomEvent>,
    delay: std::time::Duration,
    event: CustomEvent,
) -> AbortHandle {
    let (fut, abort) = abortable(async move {
        Delay::new(delay).await;
        let _ = proxy.send_event(event);
    });
    std::thread::spawn(move || {
        let _ = futures::executor::block_on(fut);
    });
    abort
}

/// Spawn an abortable loop that sends `MomentumTick` every `interval`.
fn schedule_momentum_loop(
    proxy: EventLoopProxy<CustomEvent>,
    interval: std::time::Duration,
) -> AbortHandle {
    let (fut, abort) = abortable(async move {
        loop {
            Delay::new(interval).await;
            if proxy.send_event(CustomEvent::MomentumTick).is_err() {
                break;
            }
        }
    });
    std::thread::spawn(move || {
        let _ = futures::executor::block_on(fut);
    });
    abort
}

struct App {
    proxy: EventLoopProxy<CustomEvent>,
    window: Option<Arc<Window>>,
    renderer: Option<GpuRenderer>,
    /// Concatenated instance buffer: stripes first, then ruler overlay.
    /// One draw call renders both — the shader doesn't care about ordering.
    rects: Vec<RectInstance>,
    scroll: ScrollState,
    velocity: Option<ScrollVelocity>,
    gesture_end_abort: Option<AbortHandle>,
    momentum_abort: Option<AbortHandle>,
}

impl App {
    fn new(proxy: EventLoopProxy<CustomEvent>) -> Self {
        Self {
            proxy,
            window: None,
            renderer: None,
            rects: Vec::new(),
            scroll: ScrollState::default(),
            velocity: None,
            gesture_end_abort: None,
            momentum_abort: None,
        }
    }

    fn rebuild_geometry(&mut self, width_px: f32) {
        let total = stripes_total_height();
        let mut all = build_stripes(width_px);
        all.extend(build_ruler(total));
        self.rects = all;
    }

    fn cancel_momentum(&mut self) {
        if let Some(h) = self.momentum_abort.take() {
            h.abort();
        }
    }

    fn cancel_gesture_end(&mut self) {
        if let Some(h) = self.gesture_end_abort.take() {
            h.abort();
        }
    }

    /// Apply a wheel delta and decide when momentum should kick in.
    ///
    /// Trackpads (precise pixel deltas) deliver `TouchPhase::Ended` when the
    /// user lifts their fingers — we kick momentum off that explicit signal.
    /// For non-precise wheels (mice, scroll wheels) winit reports
    /// `TouchPhase::Started` on each tick with no `Ended`, so we fall back to
    /// a silence timeout.
    ///
    /// Using `Ended` for trackpads is the fix for "scroll-fling conflict":
    /// continuous trackpad scroll never starts momentum, so there is no
    /// race between an in-flight inertia tick and an arriving wheel event.
    fn on_wheel(&mut self, applied_dy: f32, phase: TouchPhase, precise: bool) {
        // A new wheel event interrupts any in-flight inertia or pending kickoff.
        self.cancel_momentum();
        self.cancel_gesture_end();

        self.scroll.scroll_by(applied_dy);
        self.velocity = Some(ScrollVelocity::record(
            self.velocity,
            Vec2::new(0.0, applied_dy),
            Instant::now(),
        ));

        match phase {
            TouchPhase::Ended => {
                // Trackpad fingers lifted — kick momentum immediately if the
                // velocity warrants it.
                self.on_gesture_end();
            }
            TouchPhase::Cancelled => {
                // Trackpad gesture cancelled (e.g. another app took focus).
                self.velocity = None;
            }
            TouchPhase::Started | TouchPhase::Moved => {
                if !precise {
                    // Wheel mouse fallback: arm the silence timeout. Trackpads
                    // skip this — `Ended` will arrive cleanly.
                    self.gesture_end_abort = Some(schedule_once(
                        self.proxy.clone(),
                        GESTURE_END_TIMEOUT,
                        CustomEvent::GestureEnded,
                    ));
                }
            }
        }

        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }

    fn on_gesture_end(&mut self) {
        let Some(v) = self.velocity else { return };
        let speed = v.velocity.length();
        if speed < MOMENTUM_THRESHOLD {
            self.velocity = None;
            return;
        }
        // Re-anchor velocity to "now" with the clamped vector so the first
        // momentum tick computes a sensible elapsed delta.
        self.velocity = Some(ScrollVelocity {
            velocity: v.clamped_for_momentum(),
            last_update: Instant::now(),
        });
        self.momentum_abort = Some(schedule_momentum_loop(
            self.proxy.clone(),
            MOMENTUM_FRAME_INTERVAL,
        ));
    }

    fn on_momentum_tick(&mut self) {
        let Some(v) = self.velocity.as_mut() else {
            return;
        };
        let now = Instant::now();
        let elapsed = now.duration_since(v.last_update).as_secs_f32();
        v.last_update = now;
        v.velocity = decay_velocity(v.velocity, elapsed);

        if v.velocity.length() < MOMENTUM_MIN_VELOCITY {
            self.cancel_momentum();
            self.velocity = None;
            return;
        }

        let delta = v.velocity * elapsed;
        self.scroll.scroll_by(delta.y);
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }
}

impl ApplicationHandler<CustomEvent> for App {
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
        let width = renderer.size().width as f32;
        let height = renderer.size().height as f32;
        self.rebuild_geometry(width);
        self.scroll = ScrollState {
            offset_y: 0.0,
            total_size_px: stripes_total_height(),
            visible_px: height,
        };
        self.window = Some(window);
        self.renderer = Some(renderer);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.cancel_momentum();
                self.cancel_gesture_end();
                event_loop.exit();
            }
            WindowEvent::Resized(new_size) => {
                if let Some(r) = self.renderer.as_mut() {
                    r.resize(new_size);
                    self.rebuild_geometry(new_size.width as f32);
                    self.scroll.visible_px = new_size.height as f32;
                    self.scroll.scroll_by(0.0);
                }
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, phase, .. } => {
                let (precise, dy) = match delta {
                    MouseScrollDelta::PixelDelta(p) => (true, p.y as f32),
                    MouseScrollDelta::LineDelta(_, v) => (false, v * NUM_PIXELS_PER_LINE),
                };
                self.on_wheel(-dy, phase, precise);
            }
            WindowEvent::RedrawRequested => {
                if let (Some(r), Some(w)) = (self.renderer.as_ref(), self.window.as_ref()) {
                    w.pre_present_notify();
                    r.render(&self.rects, self.scroll.offset_y);
                }
            }
            _ => {}
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: CustomEvent) {
        match event {
            CustomEvent::GestureEnded => self.on_gesture_end(),
            CustomEvent::MomentumTick => self.on_momentum_tick(),
        }
    }
}

fn main() {
    let event_loop = EventLoop::<CustomEvent>::with_user_event()
        .build()
        .expect("failed to create event loop");
    let proxy = event_loop.create_proxy();
    let mut app = App::new(proxy);
    event_loop.run_app(&mut app).expect("event loop failed");
}
