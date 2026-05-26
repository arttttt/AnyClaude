//! Mini-integration of `term_core` × `term_gpu`.
//!
//! Pipes raw ANSI bytes from stdin through `term_core::VtEmulator` into a
//! GPU window rendered by `term_gpu::GpuRenderer`. This commit adds the
//! cell-to-glyph translation step: each visible row is split into runs of
//! cells that share the same SGR attributes, then each run is shaped once
//! through `TextShapeCache` and emitted as `GlyphInstance`s tinted with
//! the run's foreground colour.
//!
//! Background rects and the cursor land in the next commit.
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
//!
//! ## Cell metrics — Warp parity
//!
//! `term_core`'s grid is logically monospace; cell origin for `(row, col)`
//! is `(col × cell_width, row × cell_height)`. Following Warp's
//! `grid_size_util.rs`, cell metrics are computed in **integer physical
//! pixels**: width is `round(advance('M') × scale_factor)`, height is
//! `round(font_size × line_height_ratio × scale_factor)`. Integer
//! metrics × integer column index ⇒ every cell origin lands on a
//! physical pixel, every glyph hits `SubpixelBin::Zero`, and one atlas
//! entry per `(glyph_id, font_size, bin=Zero)` is shared across all
//! cells holding that char.
//!
//! ## Shaping — per cell, not per run
//!
//! Warp's hot path is a direct codepoint→glyph_id lookup (no shaping at
//! all for ASCII / non-combiner cells), and even when shaping is engaged
//! (ligatures, combiners) the resulting glyph advances are **discarded**
//! — each glyph is dropped at `col × cell_width`. We don't yet have
//! cosmic-text's codepoint→glyph_id lookup wired, so we approximate the
//! pattern by shaping per cell with `TextShapeCache` and only consuming
//! the glyph ID + bitmap, not the advance. Per-cell granularity is the
//! reason per-run shaping (our first attempt) blurred: in that path each
//! glyph rode on the font's natural fractional advance and landed in a
//! different `SubpixelBin` per column.
//!
//! Cell text = `cell.c` plus any `cell.extra.zerowidth` combiners. Wide
//! characters / emoji are not split into leading-cell + spacer by
//! `term_core` (the parser stores one char per cell), so a CJK glyph
//! rendered through this path may visually exceed its cell.

use std::io::Read;
use std::sync::mpsc;
use std::sync::Arc;

use cosmic_text::{FontSystem, SwashCache};
use term_core::{
    create_emulator, AnsiPalette, CellFlags, CursorState, CursorStyle, RenderSnapshot, TermColor,
    TerminalEmulator,
};
use term_gpu::{
    rasterize_glyph, FontFamily, GlyphAtlas, GlyphInstance, GpuRenderer, RectInstance, Style,
    TextShapeCache, Weight,
};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowAttributes, WindowId};

const DEFAULT_COLS: usize = 80;
const DEFAULT_ROWS: usize = 24;
const DEFAULT_SCROLLBACK: usize = 1000;
/// Logical-pixel cell height multiplier — matches the `font_size * 1.3`
/// line-height cosmic-text uses by default (see `term_gpu::text`).
const LINE_HEIGHT_RATIO: f32 = 1.3;
const FONT_SIZE: f32 = 14.0;
/// Default foreground when the cell's `fg` is `TermColor::Default`.
/// Light gray to read against the dark clear colour.
const DEFAULT_FG: [f32; 4] = [0.78, 0.78, 0.78, 1.0];
/// Cursor rectangle colour. Semi-transparent white so the cell's text
/// (drawn after rects in the same pass) still shows through a block
/// cursor instead of disappearing under it.
const CURSOR_COLOR: [f32; 4] = [0.95, 0.95, 0.95, 0.55];
/// Thickness in physical pixels for non-block cursor shapes
/// (underline / beam). Matches Warp's 2 px cursor stroke.
const CURSOR_STROKE_PHYSICAL: f32 = 2.0;

/// Custom event signalling that the stdin reader has shipped at least one
/// chunk into the channel. The handler drains the channel and requests a
/// redraw.
#[derive(Debug, Clone, Copy)]
enum CustomEvent {
    BytesArrived,
}

/// Cached cell-size measurement in **physical pixels**, rounded to
/// integers. `width = round(advance_M_physical)`, `height =
/// round(font_size × line_height_ratio × scale_factor)`. See module-level
/// docs for the rationale (Warp parity).
#[derive(Clone, Copy)]
struct CellMetrics {
    width_physical: f32,
    height_physical: f32,
}

struct App {
    window: Option<Arc<Window>>,
    renderer: Option<GpuRenderer>,
    emulator: Box<dyn TerminalEmulator>,
    bytes_rx: mpsc::Receiver<Vec<u8>>,
    font_system: FontSystem,
    swash_cache: SwashCache,
    shape_cache: TextShapeCache,
    palette: AnsiPalette,
    scale_factor: f32,
    cell_metrics: Option<CellMetrics>,
    /// Last (cols, rows) we resized the emulator to. Skip `emulator.resize`
    /// when nothing changed — guards against repeated work when winit
    /// fires multiple `Resized` events per drag frame.
    grid_size: (usize, usize),
}

impl App {
    fn new(bytes_rx: mpsc::Receiver<Vec<u8>>) -> Self {
        Self {
            window: None,
            renderer: None,
            emulator: create_emulator(DEFAULT_COLS, DEFAULT_ROWS, DEFAULT_SCROLLBACK),
            bytes_rx,
            // FontSystem scans the system font database on construction —
            // do it once. SwashCache is cheap.
            font_system: FontSystem::new(),
            swash_cache: SwashCache::new(),
            // Monospace primary family; emoji/CJK fall back through fontdb.
            shape_cache: TextShapeCache::with_family(FontFamily::Monospace),
            palette: AnsiPalette::default_dark(),
            scale_factor: 1.0,
            cell_metrics: None,
            grid_size: (DEFAULT_COLS, DEFAULT_ROWS),
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

    /// Resize the emulator's grid to fit the current window, but only
    /// when the computed `(cols, rows)` differ from the last applied
    /// size. Safe to call every frame.
    fn fit_grid_to_window(&mut self) {
        let Some(renderer) = self.renderer.as_ref() else {
            return;
        };
        let size = renderer.size();
        let metrics = self.cell_metrics();
        let cols = ((size.width as f32 / metrics.width_physical).floor() as usize).max(1);
        let rows = ((size.height as f32 / metrics.height_physical).floor() as usize).max(1);
        if (cols, rows) == self.grid_size {
            return;
        }
        self.emulator.resize(cols, rows);
        self.grid_size = (cols, rows);
    }

    /// Return cell metrics, measuring on the first call. Cell width is
    /// the rounded physical advance of `"M"` in the configured monospace
    /// family; height is the rounded physical line height. Integer
    /// physical metrics are the cornerstone of the Warp-parity rendering:
    /// every cell origin then sits on a physical pixel without further
    /// snapping. Invalidation on DPI changes lands in the resize/scale
    /// commit.
    fn cell_metrics(&mut self) -> CellMetrics {
        if let Some(m) = self.cell_metrics {
            return m;
        }
        let sf = self.scale_factor;
        let shaped = self.shape_cache.shape(
            &mut self.font_system,
            "M",
            FONT_SIZE,
            sf,
            None,
            Weight::NORMAL,
            Style::Normal,
        );
        let width_physical = shaped
            .lines
            .first()
            .and_then(|line| line.glyphs.first())
            .map(|g| g.w)
            .unwrap_or(FONT_SIZE * 0.6 * sf)
            .round()
            .max(1.0);
        let height_physical = (FONT_SIZE * LINE_HEIGHT_RATIO * sf).round().max(1.0);
        let metrics = CellMetrics {
            width_physical,
            height_physical,
        };
        self.cell_metrics = Some(metrics);
        metrics
    }

    fn on_redraw(&mut self) {
        // Pick up any pending grid resize lazily on the redraw path. Doing
        // it here (rather than in the Resized/ScaleFactorChanged handlers)
        // keeps event handlers short, lets winit coalesce a drag burst into
        // a single grid resize, and avoids re-entry into the emulator
        // while it's still being read for the previous frame's snapshot.
        self.fit_grid_to_window();
        let metrics = self.cell_metrics();
        // Split-borrow: snapshot is read-only, the rest are mutated by
        // shape/atlas insertion. Pull each field out before borrowing.
        let Self {
            window,
            renderer,
            emulator,
            font_system,
            swash_cache,
            shape_cache,
            palette,
            scale_factor,
            ..
        } = self;
        let Some(renderer) = renderer.as_mut() else {
            return;
        };
        let Some(window) = window.as_ref() else {
            return;
        };

        let snapshot = emulator.snapshot();
        let mut glyphs: Vec<GlyphInstance> = Vec::new();
        let mut rects: Vec<RectInstance> = Vec::new();

        populate_frame(
            &snapshot,
            palette,
            font_system,
            swash_cache,
            renderer.atlas_mut(),
            shape_cache,
            *scale_factor,
            metrics,
            &mut rects,
            &mut glyphs,
        );
        if let Some(cursor_rect) = build_cursor_rect(snapshot.cursor, *scale_factor, metrics) {
            rects.push(cursor_rect);
        }

        window.pre_present_notify();
        renderer.render(&rects, &glyphs, 0.0);
        shape_cache.end_frame();
    }
}

/// Walk `snapshot.rows` cell-by-cell, emitting a background `RectInstance`
/// for every cell whose effective background is not `TermColor::Default`,
/// and a per-cell shaped `GlyphInstance` set for every cell whose
/// effective foreground produces a visible glyph. Effective fg / bg
/// account for the `INVERSE` SGR flag by swapping the cell's colours.
///
/// Cell layout follows Warp parity: `(col × cell_width_physical,
/// row × cell_height_physical)`, integer × integer. Shaper advances are
/// ignored; column index alone fixes glyph X. See module docs.
#[allow(clippy::too_many_arguments)]
fn populate_frame(
    snapshot: &RenderSnapshot,
    palette: &AnsiPalette,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    atlas: &mut GlyphAtlas,
    shape_cache: &mut TextShapeCache,
    scale_factor: f32,
    metrics: CellMetrics,
    rects: &mut Vec<RectInstance>,
    glyphs: &mut Vec<GlyphInstance>,
) {
    let sf = scale_factor;
    let cell_w_logical = metrics.width_physical / sf;
    let cell_h_logical = metrics.height_physical / sf;
    let mut cell_text = String::with_capacity(8);
    for (row_idx, row) in snapshot.rows.iter().enumerate() {
        let origin_y_physical = row_idx as f32 * metrics.height_physical;
        let pos_y_logical = origin_y_physical / sf;
        for (col_idx, cell) in row.cells.iter().enumerate() {
            // Effective fg/bg accounting for INVERSE — swap the colours.
            // `INVERSE && both Default` is left as no bg/no glyph; the
            // theoretically correct behaviour (paint default-fg over
            // default-bg) has no visible difference on a blank cell with
            // our colour scheme.
            let inverse = cell.flags.contains(CellFlags::INVERSE);
            let (fg_eff, bg_eff) = if inverse {
                (cell.bg, cell.fg)
            } else {
                (cell.fg, cell.bg)
            };

            let origin_x_physical = col_idx as f32 * metrics.width_physical;
            let pos_x_logical = origin_x_physical / sf;

            // Background rect.
            if bg_eff != TermColor::Default {
                rects.push(RectInstance {
                    pos: [pos_x_logical, pos_y_logical],
                    size: [cell_w_logical, cell_h_logical],
                    color: bg_eff.to_rgba(palette),
                });
            }

            // Glyph(s) for this cell.
            let is_blank = cell.c == ' ' || cell.c == '\0';
            if is_blank && fg_eff == TermColor::Default {
                continue;
            }

            // Build the cell's shape input: base char plus any zero-width
            // combiners. Reuse one String across cells to keep per-frame
            // allocations down.
            cell_text.clear();
            cell_text.push(cell.c);
            if let Some(extra) = &cell.extra {
                for c in &extra.zerowidth {
                    cell_text.push(*c);
                }
            }

            let color = if fg_eff == TermColor::Default {
                DEFAULT_FG
            } else {
                fg_eff.to_rgba(palette)
            };

            let shaped = shape_cache.shape(
                font_system,
                &cell_text,
                FONT_SIZE,
                sf,
                None,
                Weight::NORMAL,
                Style::Normal,
            );
            for line in &shaped.lines {
                // Snap the baseline-Y to an integer physical pixel.
                let baseline_y = (origin_y_physical + line.line_y).round();
                for glyph in &line.glyphs {
                    let physical = glyph.physical((origin_x_physical, baseline_y), 1.0);
                    let Some(placed) = atlas.get_or_insert(physical.cache_key, || {
                        rasterize_glyph(font_system, swash_cache, physical.cache_key)
                    }) else {
                        continue;
                    };
                    let pos_x = (physical.x as f32 + placed.offset_x) / sf;
                    let pos_y = (physical.y as f32 - placed.offset_y) / sf;
                    glyphs.push(GlyphInstance {
                        pos: [pos_x, pos_y],
                        size: [placed.width / sf, placed.height / sf],
                        uv_min: placed.uv_min,
                        uv_max: placed.uv_max,
                        color,
                    });
                }
            }
        }
    }
}

/// Return a single `RectInstance` for the cursor, sized and positioned
/// according to `CursorStyle`. `None` when the cursor is hidden. The
/// rect is semi-transparent so that the glyph drawn in the same cell
/// remains legible under a block cursor.
fn build_cursor_rect(
    cursor: CursorState,
    scale_factor: f32,
    metrics: CellMetrics,
) -> Option<RectInstance> {
    if !cursor.visible {
        return None;
    }
    let sf = scale_factor;
    let cell_x_phys = cursor.col as f32 * metrics.width_physical;
    let cell_y_phys = cursor.row as f32 * metrics.height_physical;
    let cell_w_phys = metrics.width_physical;
    let cell_h_phys = metrics.height_physical;

    let (pos_phys, size_phys) = match cursor.style {
        CursorStyle::BlockSteady | CursorStyle::BlockBlink => {
            ([cell_x_phys, cell_y_phys], [cell_w_phys, cell_h_phys])
        }
        CursorStyle::UnderlineSteady | CursorStyle::UnderlineBlink => (
            [cell_x_phys, cell_y_phys + cell_h_phys - CURSOR_STROKE_PHYSICAL],
            [cell_w_phys, CURSOR_STROKE_PHYSICAL],
        ),
        CursorStyle::BeamSteady | CursorStyle::BeamBlink => {
            ([cell_x_phys, cell_y_phys], [CURSOR_STROKE_PHYSICAL, cell_h_phys])
        }
    };

    Some(RectInstance {
        pos: [pos_phys[0] / sf, pos_phys[1] / sf],
        size: [size_phys[0] / sf, size_phys[1] / sf],
        color: CURSOR_COLOR,
    })
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
        // Mirror the renderer's DPI so shape calls go through cosmic-text
        // at `font_size × scale_factor` physical px. Without this we'd
        // ship logical-pixel-sized glyphs to a physical-pixel framebuffer
        // and the GPU sampler would blur them by 2× on Retina.
        self.scale_factor = renderer.scale_factor();
        self.window = Some(window);
        self.renderer = Some(renderer);
        // Flush any chunks the reader queued before the window existed
        // (BytesArrived events fired then would have hit a None window).
        // Grid resize happens from the `Resized` event winit fires
        // immediately after window creation — keep this handler short so
        // we return to the event loop before macOS expects it.
        self.drain_bytes();
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
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
                // Cell metrics depend on scale_factor (shape advances are
                // measured at `font_size × scale_factor`); the next call to
                // cell_metrics() will re-measure. fit_grid_to_window runs
                // on the redraw path and will catch up to the new cell
                // size automatically.
                self.cell_metrics = None;
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
