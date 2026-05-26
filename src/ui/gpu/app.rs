//! Winit `ApplicationHandler` for the GPU UI.
//!
//! Current scope (C2): spawn a shell PTY, feed its bytes into a
//! `term_core::VtEmulator`, render the emulator's snapshot through
//! `term_gpu::populate_panel`. Keyboard / scroll / selection / clipboard
//! land in C3; header / footer chrome in C4-C5; popup overlays in
//! C6-C9. The `--gpu` CLI flag routes here for incremental
//! verification; it is removed in the C10 cutover commit.

use std::sync::Arc;

use term_core::{create_emulator, AnsiPalette, TerminalEmulator};
use term_gpu::{
    build_cursor_rect, encode_key, measure_cell_metrics, populate_panel, CellMetrics, FontFamily,
    FontSystem, GlyphInstance, GpuRenderer, PanelRect, RectInstance, SwashCache, TextShapeCache,
};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, Modifiers, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{Window, WindowAttributes, WindowId};

use crate::ui::gpu::pty::ShellPty;

const INITIAL_W: f32 = 1200.0;
const INITIAL_H: f32 = 800.0;
const FONT_SIZE: f32 = 14.0;
const LINE_HEIGHT_RATIO: f32 = 1.3;
const SCROLLBACK_LINES: usize = 1000;
const INITIAL_GRID_COLS: usize = 80;
const INITIAL_GRID_ROWS: usize = 24;

/// User event delivered to the winit loop. Drives redraws in response
/// to PTY output without polling.
#[derive(Debug, Clone, Copy)]
enum UserEvent {
    PtyBytesArrived,
}

/// Entry point for the GPU UI. Signature mirrors `ui::run` so the C10
/// cutover only flips which function `main.rs` calls.
///
/// `_backend_override` and `_claude_args` are accepted but ignored at
/// this stage — the C2 scope renders a shell PTY only. The full
/// anyclaude bootstrap (proxy + IPC + shim + claude command) lands at
/// C10.
pub fn run(_backend_override: Option<String>, _claude_args: Vec<String>) -> std::io::Result<()> {
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let proxy = event_loop.create_proxy();
    let mut app = GpuApp::new(proxy);
    event_loop
        .run_app(&mut app)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(())
}

struct GpuApp {
    proxy: EventLoopProxy<UserEvent>,
    window: Option<Arc<Window>>,
    renderer: Option<GpuRenderer>,
    scale_factor: f32,

    // Font system is owned at the app level — cosmic-text rasterizes
    // glyphs against it via the shape cache, and the swash cache holds
    // the bitmap data destined for the atlas.
    font_system: FontSystem,
    swash_cache: SwashCache,
    shape_cache: TextShapeCache,

    palette: AnsiPalette,
    cell_metrics: Option<CellMetrics>,

    // Lazily initialised in `resumed`: spawning the shell needs to know
    // the window's pixel size, which we don't have until then.
    pty: Option<ShellPty>,
    emulator: Option<Box<dyn TerminalEmulator>>,
    grid_size: (usize, usize),

    modifiers: ModifiersState,
}

impl GpuApp {
    fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        Self {
            proxy,
            window: None,
            renderer: None,
            scale_factor: 1.0,
            font_system: FontSystem::new(),
            swash_cache: SwashCache::new(),
            shape_cache: TextShapeCache::with_family(FontFamily::Monospace),
            palette: AnsiPalette::default_dark(),
            cell_metrics: None,
            pty: None,
            emulator: None,
            grid_size: (INITIAL_GRID_COLS, INITIAL_GRID_ROWS),
            modifiers: ModifiersState::empty(),
        }
    }

    fn cell_metrics(&mut self) -> CellMetrics {
        if let Some(m) = self.cell_metrics {
            return m;
        }
        let metrics = measure_cell_metrics(
            &mut self.font_system,
            &mut self.shape_cache,
            FONT_SIZE,
            self.scale_factor,
            LINE_HEIGHT_RATIO,
        );
        self.cell_metrics = Some(metrics);
        metrics
    }

    /// Compute the grid size (cols × rows) that fits inside the
    /// window's logical bounds at the current cell metrics. Both
    /// dimensions are clamped to at least 1 — a sub-cell window is
    /// degenerate but should never panic.
    fn fit_grid(&mut self) -> (usize, usize) {
        let metrics = self.cell_metrics();
        let Some(window) = self.window.as_ref() else {
            return self.grid_size;
        };
        let size = window.inner_size();
        let sf = self.scale_factor.max(0.0001);
        let cols = ((size.width as f32 / metrics.width_physical).floor() as usize).max(1);
        let rows = ((size.height as f32 / metrics.height_physical).floor() as usize).max(1);
        let _ = sf;
        (cols, rows)
    }

    /// Resync emulator + PTY to the current window size. Called from
    /// `resumed` and on `Resized`/`ScaleFactorChanged`.
    fn sync_grid_to_window(&mut self) {
        let (cols, rows) = self.fit_grid();
        if self.grid_size == (cols, rows) {
            return;
        }
        self.grid_size = (cols, rows);
        if let Some(emu) = self.emulator.as_mut() {
            emu.resize(cols, rows);
        }
        if let Some(pty) = self.pty.as_ref() {
            pty.resize(cols as u16, rows as u16);
        }
    }

    /// Drain the PTY's pending bytes into the emulator. Returns true
    /// when at least one chunk arrived (caller should request redraw).
    fn drain_pty(&mut self) -> bool {
        let Some(pty) = self.pty.as_mut() else {
            return false;
        };
        let chunks = pty.drain();
        if chunks.is_empty() {
            return false;
        }
        if let Some(emu) = self.emulator.as_mut() {
            for chunk in chunks {
                emu.process(&chunk);
            }
        }
        true
    }

    /// Render one frame: clear, populate cells, push cursor, present.
    fn redraw(&mut self) {
        let metrics = self.cell_metrics();
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let Some(emulator) = self.emulator.as_ref() else {
            return;
        };
        let size = window.inner_size();
        let sf = self.scale_factor.max(0.0001);
        let panel = PanelRect::new(0.0, 0.0, size.width as f32 / sf, size.height as f32 / sf);

        let snapshot = emulator.snapshot();
        let mut rects: Vec<RectInstance> = Vec::new();
        let mut glyphs: Vec<GlyphInstance> = Vec::new();
        populate_panel(
            &snapshot,
            panel,
            &self.palette,
            &mut self.font_system,
            &mut self.swash_cache,
            renderer.atlas_mut(),
            &mut self.shape_cache,
            FONT_SIZE,
            sf,
            metrics,
            0.0,
            &mut rects,
            &mut glyphs,
        );
        if let Some(cursor_rect) = build_cursor_rect(
            snapshot.cursor,
            snapshot.visible_start(),
            panel,
            sf,
            metrics,
            0.0,
        ) {
            rects.push(cursor_rect);
        }

        window.pre_present_notify();
        renderer.render(&rects, &glyphs, 0.0);
        self.shape_cache.end_frame();
    }
}

impl ApplicationHandler<UserEvent> for GpuApp {
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

        let (cols, rows) = self.fit_grid();
        self.grid_size = (cols, rows);
        self.emulator = Some(create_emulator(cols, rows, SCROLLBACK_LINES));

        let proxy = self.proxy.clone();
        match ShellPty::spawn(cols as u16, rows as u16, move || {
            let _ = proxy.send_event(UserEvent::PtyBytesArrived);
        }) {
            Ok(pty) => {
                self.pty = Some(pty);
            }
            Err(e) => {
                eprintln!("anyclaude: failed to spawn shell: {e}");
                event_loop.exit();
                return;
            }
        }

        window.request_redraw();
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::PtyBytesArrived => {
                if self.drain_pty() {
                    if let Some(w) = self.window.as_ref() {
                        w.request_redraw();
                    }
                }
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(new_size) => {
                if let Some(r) = self.renderer.as_mut() {
                    r.resize(new_size);
                }
                self.sync_grid_to_window();
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale_factor = scale_factor as f32;
                if let Some(r) = self.renderer.as_mut() {
                    r.set_scale_factor(self.scale_factor);
                }
                // Cell metrics depend on scale_factor; invalidate and
                // resync grid to the new physical cell size.
                self.cell_metrics = None;
                self.sync_grid_to_window();
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.update_modifiers(mods);
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed =>
            {
                // Cmd/Super combos are reserved for app-level shortcuts
                // (clipboard, quit) and lands in C3e; for now we only
                // forward when no super modifier is held. Ctrl combos
                // belong to the shell (Ctrl+C / Ctrl+D / ...) and pass
                // straight through encode_key.
                if self.modifiers.super_key() {
                    return;
                }
                let Some(bytes) = encode_key(&event.logical_key, self.modifiers) else {
                    return;
                };
                if let Some(pty) = self.pty.as_mut() {
                    if let Err(e) = pty.write(&bytes) {
                        eprintln!("anyclaude: PTY write failed: {e}");
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                self.redraw();
            }
            _ => {}
        }
    }
}

impl GpuApp {
    fn update_modifiers(&mut self, mods: Modifiers) {
        self.modifiers = mods.state();
    }
}
