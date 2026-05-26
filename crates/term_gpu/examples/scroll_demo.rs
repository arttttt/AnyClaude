//! Scroll prototype demo with pixel-based scroll + momentum integrator.
//!
//! Architecture follows `docs/design/gpu-terminal-scroll.md`:
//! - winit `MouseWheel` events feed `ScrollState` and `ScrollVelocity`
//! - after `GESTURE_END_TIMEOUT` of silence, a momentum timer kicks in
//! - the timer ticks every `MOMENTUM_FRAME_INTERVAL` and decays velocity
//! - both timers are abortable; a new wheel event cancels them

use std::sync::Arc;
use std::time::Instant;

use cosmic_text::{FontSystem, SwashCache};
use futures::future::{abortable, AbortHandle};
use futures_timer::Delay;
use glam::Vec2;
use term_gpu::{
    decay_velocity, rasterize_glyph, FontFamily, GlyphAtlas, GlyphInstance, GpuRenderer,
    RectInstance, ScrollState, ScrollVelocity, Style, TextShapeCache, Weight, GESTURE_END_TIMEOUT,
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

const LOREM: &str = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed do eiusmod \
tempor incididunt ut labore et dolore magna aliqua. Variable-width fonts \
look great with cosmic-text shaping and our subpixel-aware glyph atlas. \
\u{1F3A8} Emoji also work because the atlas is RGBA8, not single-channel.";

struct TextDraw<'a> {
    text: &'a str,
    /// Logical font size; multiplied by `scale_factor` for cosmic-text.
    font_size: f32,
    color: [f32; 4],
    /// Origin in logical pixels.
    origin_x: f32,
    origin_y: f32,
    /// Wrap width in logical pixels.
    wrap_width: Option<f32>,
    scale_factor: f32,
}

/// Per-frame context for `build_glyph_instances`. Groups scalars so the
/// builder's signature stays within clippy's `too_many_arguments` limit and
/// the culling logic is centralised in one place.
#[derive(Clone, Copy)]
struct FrameContext {
    scale_factor: f32,
    /// Scroll position in logical pixels.
    scroll_top: f32,
    /// Viewport height in logical pixels.
    viewport_height: f32,
}

impl FrameContext {
    /// Returns true if `[origin_y, origin_y + height]` overlaps the viewport.
    /// Used to skip shaping and atlas lookups for off-screen text.
    fn in_view(&self, origin_y: f32, height: f32) -> bool {
        origin_y + height > self.scroll_top
            && origin_y < self.scroll_top + self.viewport_height
    }
}

fn build_glyph_instances(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    atlas: &mut GlyphAtlas,
    shape_cache: &mut TextShapeCache,
    out: &mut Vec<GlyphInstance>,
    frame: FrameContext,
) {
    let sf = frame.scale_factor;

    // 1. Banner at the very top of scroll space.
    let banner_height = 18.0 * 1.5;
    if frame.in_view(4.0, banner_height) {
        shape_text_into(
            font_system,
            swash_cache,
            atlas,
            shape_cache,
            out,
            TextDraw {
                text: "term_gpu scroll demo \u{2022} cosmic-text shaping \u{1F680}",
                font_size: 18.0,
                color: [0.95, 0.95, 0.95, 1.0],
                origin_x: STRIPE_X_MARGIN + 8.0,
                origin_y: 4.0,
                wrap_width: None,
                scale_factor: sf,
            },
        );
    }

    // 2. Lorem ipsum paragraph below the banner, wrapped. Pessimistic height
    // estimate is fine — culling only matters when the user scrolls past it.
    let lorem_height = 14.0 * 1.5 * 8.0;
    if frame.in_view(36.0, lorem_height) {
        shape_text_into(
            font_system,
            swash_cache,
            atlas,
            shape_cache,
            out,
            TextDraw {
                text: LOREM,
                font_size: 14.0,
                color: [0.05, 0.05, 0.08, 1.0],
                origin_x: STRIPE_X_MARGIN + 8.0,
                origin_y: 36.0,
                wrap_width: Some(700.0),
                scale_factor: sf,
            },
        );
    }

    // 3. "Row N" labels for every 10th stripe. Every 100th gets an emoji.
    // On a 720 logical px viewport this culls ~90 of 100 labels per frame.
    let row_height = 12.0 * 1.5;
    for i in (0..STRIPE_COUNT).step_by(10) {
        let y = i as f32 * (STRIPE_HEIGHT + STRIPE_GAP) + 4.0;
        if !frame.in_view(y, row_height) {
            continue;
        }
        let text = if i > 0 && i % 100 == 0 {
            format!("Row {i} \u{1F389}")
        } else {
            format!("Row {i}")
        };
        shape_text_into(
            font_system,
            swash_cache,
            atlas,
            shape_cache,
            out,
            TextDraw {
                text: &text,
                font_size: 12.0,
                color: [0.05, 0.05, 0.08, 1.0],
                origin_x: STRIPE_X_MARGIN + 12.0,
                origin_y: y,
                wrap_width: None,
                scale_factor: sf,
            },
        );
    }
}

/// Shape `draw.text` at `draw.origin_{x,y}` (top-left in **logical**
/// scroll coordinates), look each glyph up in the atlas, and push a
/// `GlyphInstance` per glyph into `out`.
///
/// DPI: we shape with `font_size * scale_factor` so cosmic-text rasterizes
/// at the display's physical pixel density, then divide the returned
/// positions back to logical so they match `GlyphInstance.pos` (which the
/// shader multiplies by `scale_factor` before NDC).
fn shape_text_into(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    atlas: &mut GlyphAtlas,
    shape_cache: &mut TextShapeCache,
    out: &mut Vec<GlyphInstance>,
    draw: TextDraw<'_>,
) {
    let sf = draw.scale_factor;
    let shaped = shape_cache.shape(
        font_system,
        draw.text,
        draw.font_size,
        sf,
        draw.wrap_width,
        Weight::NORMAL,
        Style::Normal,
    );

    let origin_physical_x = draw.origin_x * sf;
    let origin_physical_y = draw.origin_y * sf;

    for line in &shaped.lines {
        for glyph in &line.glyphs {
            // physical() bins the fractional position into subpixel cache
            // key variants for us. See spec §5.6.
            let physical = glyph.physical(
                (origin_physical_x, origin_physical_y + line.line_y),
                1.0,
            );
            let placed = atlas.get_or_insert(physical.cache_key, || {
                rasterize_glyph(font_system, swash_cache, physical.cache_key)
            });
            let Some(placed) = placed else {
                continue;
            };
            // Convert physical positions and size back to logical for the
            // GlyphInstance (the shader re-applies scale_factor).
            let pos_x = (physical.x as f32 + placed.offset_x) / sf;
            let pos_y = (physical.y as f32 - placed.offset_y) / sf;
            out.push(GlyphInstance {
                pos: [pos_x, pos_y],
                size: [placed.width / sf, placed.height / sf],
                uv_min: placed.uv_min,
                uv_max: placed.uv_max,
                color: draw.color,
            });
        }
    }
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
    font_system: FontSystem,
    swash_cache: SwashCache,
    shape_cache: TextShapeCache,
    /// Mirrors `window.scale_factor()`. Updated on ScaleFactorChanged.
    scale_factor: f32,
}

impl App {
    fn new(proxy: EventLoopProxy<CustomEvent>) -> Self {
        // FontSystem scans system fonts on first construction — heavy.
        // Keep one for the lifetime of the App.
        Self {
            proxy,
            window: None,
            renderer: None,
            rects: Vec::new(),
            scroll: ScrollState::default(),
            velocity: None,
            gesture_end_abort: None,
            momentum_abort: None,
            font_system: FontSystem::new(),
            swash_cache: SwashCache::new(),
            // Explicit sans-serif UI family; emoji and CJK fall back through
            // the system font database automatically (see text.rs docs).
            shape_cache: TextShapeCache::with_family(FontFamily::SansSerif),
            scale_factor: 1.0,
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

    fn on_redraw(&mut self) {
        // Destructure to split-borrow font_system, swash_cache, and the
        // renderer's atlas simultaneously (each is a distinct field).
        let Self {
            renderer,
            window,
            rects,
            scroll,
            font_system,
            swash_cache,
            shape_cache,
            scale_factor,
            ..
        } = self;
        let Some(renderer) = renderer.as_mut() else {
            return;
        };
        let Some(window) = window.as_ref() else {
            return;
        };

        let mut glyphs = Vec::new();
        let frame_ctx = FrameContext {
            scale_factor: *scale_factor,
            scroll_top: scroll.offset_y,
            viewport_height: scroll.visible_px,
        };
        build_glyph_instances(
            font_system,
            swash_cache,
            renderer.atlas_mut(),
            shape_cache,
            &mut glyphs,
            frame_ctx,
        );

        window.pre_present_notify();
        renderer.render(rects, &glyphs, scroll.offset_y);
        shape_cache.end_frame();
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
        self.scale_factor = renderer.scale_factor();
        // Width passed to ruler/stripes is in logical pixels.
        let logical_width = renderer.size().width as f32 / self.scale_factor;
        let logical_height = renderer.size().height as f32 / self.scale_factor;
        self.rebuild_geometry(logical_width);
        self.scroll = ScrollState {
            offset_y: 0.0,
            total_size_px: stripes_total_height(),
            visible_px: logical_height,
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
                    let logical_w = new_size.width as f32 / self.scale_factor;
                    let logical_h = new_size.height as f32 / self.scale_factor;
                    self.rebuild_geometry(logical_w);
                    self.scroll.visible_px = logical_h;
                    self.scroll.scroll_by(0.0);
                }
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale_factor = scale_factor as f32;
                if let Some(r) = self.renderer.as_mut() {
                    r.set_scale_factor(self.scale_factor);
                    let logical_w = r.size().width as f32 / self.scale_factor;
                    let logical_h = r.size().height as f32 / self.scale_factor;
                    self.rebuild_geometry(logical_w);
                    self.scroll.visible_px = logical_h;
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
            WindowEvent::RedrawRequested => self.on_redraw(),
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
