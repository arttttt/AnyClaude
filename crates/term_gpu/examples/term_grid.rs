//! Multi-panel virtual terminal demo.
//!
//! Bootstrap commit: a single panel hosts a real shell PTY. The
//! emulator consumes the shell's output and the GPU renderer paints
//! the resulting cell grid. Keyboard input to the PTY, multi-panel
//! split/close, and per-panel resize propagation land in subsequent
//! commits.
//!
//! ## Run
//!
//! ```bash
//! cargo run -p term_gpu --example term_grid --release
//! ```
//!
//! ## Status
//!
//! Esc / any key are dropped (no input routing yet); use `Cmd+Q` (or
//! `Ctrl+Q` on non-macOS) to quit. Shell output appears as it
//! arrives — most shells print the prompt at start, so the window is
//! not visually empty.

use std::collections::HashMap;
use std::io::Read;
use std::sync::mpsc;
use std::sync::Arc;

use cosmic_text::{FontSystem, SwashCache};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use term_core::{
    create_emulator, AnsiPalette, CellFlags, CursorState, CursorStyle, RenderSnapshot, TermColor,
    TerminalEmulator,
};
use term_gpu::{
    rasterize_glyph, FontFamily, GlyphAtlas, GlyphInstance, GpuRenderer, RectInstance,
    TextShapeCache,
};
use term_layout::{PanelId, PanelTree, Rect};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState};
use winit::window::{Window, WindowAttributes, WindowId};

const INITIAL_W: f32 = 960.0;
const INITIAL_H: f32 = 600.0;
const FONT_SIZE: f32 = 14.0;
const LINE_HEIGHT_RATIO: f32 = 1.3;
const DEFAULT_FG: [f32; 4] = [0.78, 0.78, 0.78, 1.0];
const CURSOR_COLOR: [f32; 4] = [0.95, 0.95, 0.95, 0.55];
const CURSOR_STROKE_PHYSICAL: f32 = 2.0;
const INITIAL_GRID_COLS: usize = 80;
const INITIAL_GRID_ROWS: usize = 24;
const SCROLLBACK_LINES: usize = 1000;

#[derive(Debug, Clone, Copy)]
enum CustomEvent {
    /// At least one panel's reader thread queued new bytes.
    BytesArrived(PanelId),
}

#[derive(Clone, Copy)]
struct CellMetrics {
    width_physical: f32,
    height_physical: f32,
}

struct PanelState {
    emulator: Box<dyn TerminalEmulator>,
    bytes_rx: mpsc::Receiver<Vec<u8>>,
    /// PTY master kept alive — dropping it closes the shell. We don't
    /// touch it directly in this commit; subsequent commits read/write
    /// through it.
    _master: Box<dyn MasterPty + Send>,
}

struct App {
    window: Option<Arc<Window>>,
    renderer: Option<GpuRenderer>,
    tree: PanelTree,
    panels: HashMap<PanelId, PanelState>,
    font_system: FontSystem,
    swash_cache: SwashCache,
    shape_cache: TextShapeCache,
    palette: AnsiPalette,
    scale_factor: f32,
    cell_metrics: Option<CellMetrics>,
    modifiers: ModifiersState,
    proxy: EventLoopProxy<CustomEvent>,
}

impl App {
    fn new(proxy: EventLoopProxy<CustomEvent>) -> Self {
        Self {
            window: None,
            renderer: None,
            tree: PanelTree::new(INITIAL_W, INITIAL_H),
            panels: HashMap::new(),
            font_system: FontSystem::new(),
            swash_cache: SwashCache::new(),
            shape_cache: TextShapeCache::with_family(FontFamily::Monospace),
            palette: AnsiPalette::default_dark(),
            scale_factor: 1.0,
            cell_metrics: None,
            modifiers: ModifiersState::empty(),
            proxy,
        }
    }

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

    /// Spawn a PTY running `$SHELL` (or `/bin/sh` as a fallback),
    /// install a reader thread that ships bytes through `mpsc` and
    /// signals the event loop, and return the per-panel state. The
    /// PanelId the panel will be installed under is needed so the
    /// reader thread can tag its `BytesArrived` events.
    fn spawn_panel(
        &self,
        panel_id: PanelId,
        cols: usize,
        rows: usize,
    ) -> Result<PanelState, Box<dyn std::error::Error>> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows: rows as u16,
            cols: cols as u16,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let cmd = CommandBuilder::new(shell);
        let _child = pair.slave.spawn_command(cmd)?;
        // Drop slave so the PTY closes when the shell exits.
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader()?;
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                        if proxy.send_event(CustomEvent::BytesArrived(panel_id)).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        let emulator = create_emulator(cols, rows, SCROLLBACK_LINES);
        Ok(PanelState {
            emulator,
            bytes_rx: rx,
            _master: pair.master,
        })
    }

    fn drain_panel(&mut self, id: PanelId) {
        let Some(panel) = self.panels.get_mut(&id) else {
            return;
        };
        while let Ok(chunk) = panel.bytes_rx.try_recv() {
            panel.emulator.process(&chunk);
        }
        // Discard PTY responses — no writer wired up yet. Commit 2
        // forwards keyboard input and could ship DA/DSR replies back.
        let _ = panel.emulator.take_responses();
    }

    fn on_redraw(&mut self) {
        let metrics = self.cell_metrics();
        let Self {
            window,
            renderer,
            tree,
            panels,
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

        let mut rects: Vec<RectInstance> = Vec::new();
        let mut glyphs: Vec<GlyphInstance> = Vec::new();
        let sf = *scale_factor;
        let focused = tree.focus();
        for (id, panel_rect) in tree.panels() {
            let Some(panel) = panels.get(&id) else {
                continue;
            };
            let snapshot = panel.emulator.snapshot();
            populate_panel(
                &snapshot,
                panel_rect,
                palette,
                font_system,
                swash_cache,
                renderer.atlas_mut(),
                shape_cache,
                sf,
                metrics,
                &mut rects,
                &mut glyphs,
            );
            if id == focused {
                if let Some(cr) = build_cursor_rect(snapshot.cursor, panel_rect, sf, metrics) {
                    rects.push(cr);
                }
            }
        }

        window.pre_present_notify();
        renderer.render(&rects, &glyphs, 0.0);
        shape_cache.end_frame();
    }
}

impl ApplicationHandler<CustomEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = WindowAttributes::default()
            .with_title("term_grid")
            .with_inner_size(LogicalSize::new(INITIAL_W, INITIAL_H));
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create window"),
        );
        let renderer = GpuRenderer::new(window.clone());
        self.scale_factor = renderer.scale_factor();
        self.window = Some(window);
        self.renderer = Some(renderer);

        // Spawn the initial shell into the only existing panel.
        let id = self.tree.focus();
        match self.spawn_panel(id, INITIAL_GRID_COLS, INITIAL_GRID_ROWS) {
            Ok(state) => {
                self.panels.insert(id, state);
            }
            Err(e) => {
                eprintln!("term_grid: failed to spawn shell: {e}");
                event_loop.exit();
                return;
            }
        }
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
                let logical_w = new_size.width as f32 / self.scale_factor;
                let logical_h = new_size.height as f32 / self.scale_factor;
                self.tree.resize(logical_w, logical_h);
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale_factor = scale_factor as f32;
                if let Some(r) = self.renderer.as_mut() {
                    r.set_scale_factor(self.scale_factor);
                }
                self.cell_metrics = None;
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                // Cmd/Ctrl + Q quits. Other keys are dropped — the next
                // commit wires keyboard input through to the PTY.
                let cmd = self.modifiers.super_key() || self.modifiers.control_key();
                if cmd {
                    if let Key::Character(c) = &event.logical_key {
                        if c.eq_ignore_ascii_case("q") {
                            event_loop.exit();
                            return;
                        }
                    }
                }
                let _ = event;
            }
            WindowEvent::RedrawRequested => self.on_redraw(),
            _ => {}
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: CustomEvent) {
        match event {
            CustomEvent::BytesArrived(id) => {
                self.drain_panel(id);
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn populate_panel(
    snapshot: &RenderSnapshot,
    panel_rect: Rect,
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
    let panel_origin_x_physical = panel_rect.x * sf;
    let panel_origin_y_physical = panel_rect.y * sf;
    let mut cell_text = String::with_capacity(8);

    for (row_idx, row) in snapshot.rows.iter().enumerate() {
        let row_y_phys = row_idx as f32 * metrics.height_physical;
        for (col_idx, cell) in row.cells.iter().enumerate() {
            let inverse = cell.flags.contains(CellFlags::INVERSE);
            let (fg_eff, bg_eff) = if inverse {
                (cell.bg, cell.fg)
            } else {
                (cell.fg, cell.bg)
            };

            let col_x_phys = col_idx as f32 * metrics.width_physical;
            let pos_x_logical = (panel_origin_x_physical + col_x_phys) / sf;
            let pos_y_logical = (panel_origin_y_physical + row_y_phys) / sf;

            if bg_eff != TermColor::Default {
                rects.push(RectInstance {
                    pos: [pos_x_logical, pos_y_logical],
                    size: [cell_w_logical, cell_h_logical],
                    color: bg_eff.to_rgba(palette),
                });
            }

            let is_blank = cell.c == ' ' || cell.c == '\0';
            if is_blank && fg_eff == TermColor::Default {
                continue;
            }

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

            let cell_origin_x_phys = panel_origin_x_physical + col_x_phys;
            let cell_origin_y_phys = panel_origin_y_physical + row_y_phys;

            let shaped = shape_cache.shape(font_system, &cell_text, FONT_SIZE, sf, None);
            for line in &shaped.lines {
                let baseline_y = (cell_origin_y_phys + line.line_y).round();
                for glyph in &line.glyphs {
                    let physical = glyph.physical((cell_origin_x_phys, baseline_y), 1.0);
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

fn build_cursor_rect(
    cursor: CursorState,
    panel_rect: Rect,
    scale_factor: f32,
    metrics: CellMetrics,
) -> Option<RectInstance> {
    if !cursor.visible {
        return None;
    }
    let sf = scale_factor;
    let cell_x_phys = panel_rect.x * sf + cursor.col as f32 * metrics.width_physical;
    let cell_y_phys = panel_rect.y * sf + cursor.row as f32 * metrics.height_physical;
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
        CursorStyle::BeamSteady | CursorStyle::BeamBlink => (
            [cell_x_phys, cell_y_phys],
            [CURSOR_STROKE_PHYSICAL, cell_h_phys],
        ),
    };
    Some(RectInstance {
        pos: [pos_phys[0] / sf, pos_phys[1] / sf],
        size: [size_phys[0] / sf, size_phys[1] / sf],
        color: CURSOR_COLOR,
    })
}

fn main() {
    let event_loop = EventLoop::<CustomEvent>::with_user_event()
        .build()
        .expect("failed to create event loop");
    let proxy = event_loop.create_proxy();
    let mut app = App::new(proxy);
    event_loop.run_app(&mut app).expect("event loop failed");
}
