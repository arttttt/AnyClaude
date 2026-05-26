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
use term_core::{create_emulator, AnsiPalette, RenderSnapshot, TermColor, TerminalEmulator};
use term_gpu::{
    rasterize_glyph, FontFamily, GlyphAtlas, GlyphInstance, GpuRenderer, RectInstance,
    TextShapeCache,
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
        let shaped = self
            .shape_cache
            .shape(&mut self.font_system, "M", FONT_SIZE, sf, None);
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
        // Backgrounds and cursor land in the next commit.
        let rects: Vec<RectInstance> = Vec::new();

        snapshot_to_glyphs(
            &snapshot,
            palette,
            font_system,
            swash_cache,
            renderer.atlas_mut(),
            shape_cache,
            *scale_factor,
            metrics,
            &mut glyphs,
        );

        window.pre_present_notify();
        renderer.render(&rects, &glyphs, 0.0);
        shape_cache.end_frame();
    }
}

/// Walk `snapshot.rows` cell-by-cell. Each non-empty cell is shaped on
/// its own (base char + any combining marks from `cell.extra.zerowidth`)
/// and emitted as `GlyphInstance`s placed at `(col × cell_width, row ×
/// cell_height)` in integer physical pixels. The shaper's advances are
/// ignored — column index × integer `cell_width_physical` is the only
/// X source. This mirrors Warp's `paint_line` / `render_cell_glyph`
/// behaviour (see commit message for the research summary).
#[allow(clippy::too_many_arguments)]
fn snapshot_to_glyphs(
    snapshot: &RenderSnapshot,
    palette: &AnsiPalette,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    atlas: &mut GlyphAtlas,
    shape_cache: &mut TextShapeCache,
    scale_factor: f32,
    metrics: CellMetrics,
    out: &mut Vec<GlyphInstance>,
) {
    let sf = scale_factor;
    let mut cell_text = String::with_capacity(8);
    for (row_idx, row) in snapshot.rows.iter().enumerate() {
        let origin_y_physical = row_idx as f32 * metrics.height_physical;
        for (col_idx, cell) in row.cells.iter().enumerate() {
            // Skip cells with no visible glyph and default fg. Background
            // rects (which would justify drawing a blank cell) land in the
            // next commit.
            let is_blank = cell.c == ' ' || cell.c == '\0';
            if is_blank && cell.fg == TermColor::Default {
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

            let color = if cell.fg == TermColor::Default {
                DEFAULT_FG
            } else {
                cell.fg.to_rgba(palette)
            };
            let origin_x_physical = col_idx as f32 * metrics.width_physical;

            let shaped = shape_cache.shape(font_system, &cell_text, FONT_SIZE, sf, None);
            for line in &shaped.lines {
                // Snap the baseline-Y to an integer physical pixel. Without
                // this, `origin_y_physical + line.line_y` carries the
                // fractional part of `line.line_y` (cosmic-text returns it
                // in physical units, but never rounded), which pushes
                // `glyph.physical()` into a non-zero `SubpixelBin::Y`. The
                // rasterised image is then shifted by 1/4 px vertically and
                // every row picks a slightly different image — the
                // residual softness we observed after fixing X.
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
                    out.push(GlyphInstance {
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
