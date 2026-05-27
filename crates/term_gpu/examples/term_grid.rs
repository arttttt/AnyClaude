//! Multi-panel virtual terminal demo.
//!
//! Each panel owns a real shell PTY. Keyboard input goes to the
//! focused panel; `Cmd+D` / `Cmd+Shift+D` / `Cmd+W` mutate the
//! `PanelTree`; mouse click focuses; left-drag near a divider
//! resizes it. Exiting the shell (`Ctrl+D`, `exit`) closes the
//! corresponding panel; closing the last panel ends the demo.
//!
//! Per-panel PTY resize: every tree mutation (window resize, split,
//! close, divider drag, DPI change) runs `sync_panels_to_tree`, which
//! walks the leaves and resizes each emulator + PTY master to fit its
//! current bounds (cols/rows computed by integer floor of
//! `rect_physical / cell_metrics`). Shells see SIGWINCH and reflow
//! their output (`tput cols` reports the right value, `vim` /
//! `htop` redraw to fit).
//!
//! ## Run
//!
//! ```bash
//! cargo run -p term_gpu --example term_grid --release
//! ```
//!
//! ## Shortcuts
//!
//! - `Cmd+Q` — quit the demo.
//! - `Cmd+D` — vertical split (new shell on the right).
//! - `Cmd+Shift+D` — horizontal split (new shell on the bottom).
//! - `Cmd+W` — close the focused panel.
//! - Mouse click on a panel — focus it.
//! - Mouse left-drag near a divider — resize.
//! - Everything else — forwarded to the focused shell.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Instant;

use cosmic_text::{FontSystem, SwashCache};
use futures::future::{abortable, AbortHandle};
use futures_timer::Delay;
use glam::Vec2;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use term_clipboard::{
    get_image_filepaths_from_paths, pick_best_image, save_image_to_temp,
    should_insert_text_on_paste, Clipboard, ClipboardContent,
};
use term_core::{create_emulator, AnsiPalette, MouseMode, TerminalEmulator};
use term_gpu::{
    build_cursor_rect, decay_velocity, encode_key, encode_paste, expand_line, expand_word,
    measure_cell_metrics, populate_panel, push_selection_rects, selection_to_text,
    shell_quote_path, CellMetrics, CellPoint, FontFamily, GlyphInstance, GpuRenderer, PanelRect,
    RectInstance, RenderLayer, ScrollState, ScrollVelocity, Selection, TextShapeCache,
    GESTURE_END_TIMEOUT, MOMENTUM_FRAME_INTERVAL, MOMENTUM_MIN_VELOCITY, MOMENTUM_THRESHOLD,
    NUM_PIXELS_PER_LINE,
};
use term_layout::{BranchId, Divider, PanelId, PanelTree, Rect, Split};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::window::{Window, WindowAttributes, WindowId};

const INITIAL_W: f32 = 960.0;
const INITIAL_H: f32 = 600.0;
const FONT_SIZE: f32 = 14.0;
const INITIAL_GRID_COLS: usize = 80;
const INITIAL_GRID_ROWS: usize = 24;
const SCROLLBACK_LINES: usize = 1000;
/// Float fuzz when checking "are we at the very bottom of scrollback".
/// Floats accumulated from wheel deltas rarely land on an exact integer
/// pixel; this tolerates ~half a logical px of slop so follow mode
/// engages reliably.
const SCROLL_BOTTOM_EPSILON: f32 = 0.5;
/// Logical-pixel tolerance for "did the mouse click on a divider?".
const DIVIDER_HIT_TOLERANCE: f32 = 6.0;
/// Focus border thickness and colour (alpha-blended, slim).
const FOCUS_BORDER: f32 = 2.0;
const FOCUS_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 0.35];

/// Maximum elapsed time between consecutive mouse presses at the same
/// cell for them to count as a double/triple click. macOS's system
/// default is ~500 ms; 400 ms is a comfortable middle ground.
const MULTI_CLICK_THRESHOLD_MS: u128 = 400;



#[derive(Debug, Clone, Copy)]
enum CustomEvent {
    /// At least one panel's reader thread queued new bytes.
    BytesArrived(PanelId),
    /// A panel's PTY reader hit EOF — the shell exited.
    PanelExited(PanelId),
    /// Wheel-mouse silence timeout elapsed for the currently scrolling
    /// panel — start momentum if velocity is high enough.
    GestureEnded(PanelId),
    /// One frame of inertia decay for the scrolling panel.
    MomentumTick(PanelId),
}

#[derive(Debug, Clone, Copy)]
struct DragState {
    branch: BranchId,
    split: Split,
    bounds: Rect,
}

struct PanelState {
    emulator: Box<dyn TerminalEmulator>,
    bytes_rx: mpsc::Receiver<Vec<u8>>,
    writer: Box<dyn Write + Send>,
    /// PTY master — used for `resize()` calls; dropping it closes the
    /// shell.
    master: Box<dyn MasterPty + Send>,
    /// Last `(cols, rows)` the emulator + PTY were resized to. Lets
    /// `sync_panels_to_tree` skip work when nothing changed.
    grid_size: (usize, usize),
    /// Pixel-precise scroll offset into the panel's scrollback. 0.0 =
    /// bottom (cursor visible); larger values mean we're looking
    /// further up into history.
    scroll: ScrollState,
    /// Active text selection (set by mouse drag, cleared on Esc, new
    /// click, PTY bytes, or column resize). `None` when the panel has
    /// nothing selected.
    selection: Option<Selection>,
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
    cursor: Option<(f32, f32)>,
    drag: Option<DragState>,
    /// Wheel events route here until either the gesture ends or a new
    /// panel takes over. Momentum and gesture-end timers fire against
    /// this id so a panel close / focus change cancels them cleanly.
    scrolling_panel: Option<PanelId>,
    scroll_velocity: Option<ScrollVelocity>,
    momentum_abort: Option<AbortHandle>,
    gesture_end_abort: Option<AbortHandle>,
    /// While the left mouse button is held over a panel cell (not on
    /// a divider) and the emulator is not in mouse-reporting mode,
    /// CursorMoved events update this panel's selection.cursor.
    dragging_selection: Option<PanelId>,
    /// Tracks consecutive clicks at the same cell for double/triple
    /// click detection. Cleared when the next click lands elsewhere
    /// or after `MULTI_CLICK_THRESHOLD_MS` of inactivity.
    last_click: Option<LastClick>,
    /// System clipboard handle. `MacClipboard` on macOS,
    /// `InMemoryClipboard` elsewhere (the demo's only supported
    /// platform today is macOS — see `term_clipboard::MacClipboard`).
    clipboard: Box<dyn Clipboard>,
    proxy: EventLoopProxy<CustomEvent>,
}

#[derive(Debug, Clone, Copy)]
struct LastClick {
    time: Instant,
    panel: PanelId,
    point: CellPoint,
    count: u32,
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
            cursor: None,
            drag: None,
            scrolling_panel: None,
            scroll_velocity: None,
            momentum_abort: None,
            gesture_end_abort: None,
            dragging_selection: None,
            last_click: None,
            clipboard: make_clipboard(),
            proxy,
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
        );
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
        let writer = pair.master.take_writer()?;
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
            // PTY closed (shell exited or master dropped) — tell the
            // event loop so the panel can be torn down.
            let _ = proxy.send_event(CustomEvent::PanelExited(panel_id));
        });

        let emulator = create_emulator(cols, rows, SCROLLBACK_LINES);
        Ok(PanelState {
            emulator,
            bytes_rx: rx,
            writer,
            master: pair.master,
            grid_size: (cols, rows),
            scroll: ScrollState::default(),
            selection: None,
        })
    }

    /// Walk the panel tree and resize each panel's emulator + PTY
    /// master to fit its current bounds. No-op when nothing changed
    /// — the per-panel `grid_size` cache guards against redundant
    /// SIGWINCH bursts during a drag.
    fn sync_panels_to_tree(&mut self) {
        let metrics = self.cell_metrics();
        let sf = self.scale_factor;
        let leaves = self.tree.panels();
        for (id, rect) in leaves {
            let Some(panel) = self.panels.get_mut(&id) else {
                continue;
            };
            let cols =
                ((rect.w * sf / metrics.width_physical).floor() as usize).max(1);
            let rows =
                ((rect.h * sf / metrics.height_physical).floor() as usize).max(1);
            if panel.grid_size == (cols, rows) {
                continue;
            }
            // Reflow will rearrange row positions — any active
            // selection is clobbered. Clear it so the user sees a
            // clean slate after the resize settles.
            panel.selection = None;
            panel.emulator.resize(cols, rows);
            let _ = panel.master.resize(PtySize {
                rows: rows as u16,
                cols: cols as u16,
                pixel_width: 0,
                pixel_height: 0,
            });
            panel.grid_size = (cols, rows);
        }
    }

    /// Hit-test logical coordinates against the panel tree. `None` when
    /// the position is between panels (on a divider).
    fn panel_at(&self, x: f32, y: f32) -> Option<PanelId> {
        self.tree
            .panels()
            .into_iter()
            .find(|(_, rect)| {
                x >= rect.x
                    && x < rect.x + rect.w
                    && y >= rect.y
                    && y < rect.y + rect.h
            })
            .map(|(id, _)| id)
    }

    /// Map a logical-coordinate point to the absolute cell (row in
    /// `RenderSnapshot::rows`, column) at that screen position inside
    /// the named panel. Returns `None` if the panel is gone.
    fn cell_at_panel(&mut self, panel_id: PanelId, x: f32, y: f32) -> Option<CellPoint> {
        let metrics = self.cell_metrics();
        let panel_rect = self
            .tree
            .panels()
            .into_iter()
            .find_map(|(id, r)| if id == panel_id { Some(r) } else { None })?;
        let panel = self.panels.get(&panel_id)?;
        let sf = self.scale_factor;
        let cell_w_logical = metrics.width_physical / sf;
        let cell_h_logical = metrics.height_physical / sf;
        if cell_w_logical <= 0.0 || cell_h_logical <= 0.0 {
            return None;
        }
        let local_x = (x - panel_rect.x).max(0.0);
        let local_y = (y - panel_rect.y).max(0.0);

        let snap = panel.emulator.snapshot();
        let total_rows = snap.rows.len();
        let visible_rows = snap.visible_rows;
        // Inverse of populate_panel's row positioning:
        //   row_y_logical = row_idx * cell_h - baseline_offset + scroll_offset
        //   row_idx = (row_y_logical + baseline_offset - scroll_offset) / cell_h
        let baseline_offset = total_rows.saturating_sub(visible_rows) as f32 * cell_h_logical;
        let row_unclamped =
            ((local_y + baseline_offset - panel.scroll.offset_y) / cell_h_logical).floor();
        let row = row_unclamped
            .clamp(0.0, total_rows.saturating_sub(1) as f32) as usize;
        let cols = snap.rows.first().map(|r| r.cells.len()).unwrap_or(0);
        let col_unclamped = (local_x / cell_w_logical).floor();
        let col = col_unclamped.clamp(0.0, cols.saturating_sub(1) as f32) as usize;
        Some(CellPoint { row, col })
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

    /// Refresh `ScrollState` totals for a panel from its current emulator
    /// snapshot and bounds. Called before applying any scroll delta to
    /// make sure clamping uses up-to-date geometry.
    fn refresh_scroll_geometry(&mut self, id: PanelId) {
        let Some(rect) = self.tree.panels().into_iter().find_map(
            |(pid, r)| if pid == id { Some(r) } else { None },
        ) else {
            return;
        };
        let metrics = self.cell_metrics();
        let cell_h_logical = metrics.height_physical / self.scale_factor;
        let Some(panel) = self.panels.get_mut(&id) else {
            return;
        };
        let snap = panel.emulator.snapshot();
        panel.scroll.total_size_px = snap.rows.len() as f32 * cell_h_logical;
        panel.scroll.visible_px = rect.h;
        let max = panel.scroll.max_offset();
        if panel.scroll.offset_y > max {
            panel.scroll.offset_y = max;
        }
    }

    /// Apply a wheel delta to the panel under the cursor. Trackpad
    /// `TouchPhase::Ended` kicks momentum immediately; for non-precise
    /// wheels (mice) a silence timeout falls back to the same path.
    /// Mirrors `scroll_demo::App::on_wheel` per-panel.
    fn on_wheel(&mut self, dy: f32, phase: TouchPhase, precise: bool) {
        let Some((x, y)) = self.cursor else {
            return;
        };
        let Some(target) = self.panel_at(x, y) else {
            return;
        };
        // A new wheel event interrupts any in-flight momentum and pending kickoff.
        self.cancel_momentum();
        self.cancel_gesture_end();

        // Switching panels mid-flight invalidates the previous velocity sample.
        if self.scrolling_panel != Some(target) {
            self.scroll_velocity = None;
        }
        self.scrolling_panel = Some(target);

        self.refresh_scroll_geometry(target);
        if let Some(panel) = self.panels.get_mut(&target) {
            panel.scroll.scroll_by(dy);
        }
        self.scroll_velocity = Some(ScrollVelocity::record(
            self.scroll_velocity,
            Vec2::new(0.0, dy),
            Instant::now(),
        ));

        match phase {
            TouchPhase::Ended => {
                self.on_gesture_end();
            }
            TouchPhase::Cancelled => {
                self.scroll_velocity = None;
            }
            TouchPhase::Started | TouchPhase::Moved => {
                if !precise {
                    self.gesture_end_abort = Some(schedule_once(
                        self.proxy.clone(),
                        GESTURE_END_TIMEOUT,
                        CustomEvent::GestureEnded(target),
                    ));
                }
            }
        }

        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }

    fn on_gesture_end(&mut self) {
        let Some(target) = self.scrolling_panel else { return };
        let Some(v) = self.scroll_velocity else { return };
        let speed = v.velocity.length();
        if speed < MOMENTUM_THRESHOLD {
            self.scroll_velocity = None;
            return;
        }
        self.scroll_velocity = Some(ScrollVelocity {
            velocity: v.clamped_for_momentum(),
            last_update: Instant::now(),
        });
        self.momentum_abort = Some(schedule_momentum_loop(
            self.proxy.clone(),
            MOMENTUM_FRAME_INTERVAL,
            target,
        ));
    }

    /// Read the system clipboard and paste it into the focused
    /// panel's PTY. Mirrors Warp's `process_paste_event`
    /// (`app/src/terminal/input.rs:10573`):
    ///
    ///   1. If `should_insert_text_on_paste(&content)` is true,
    ///      include `content.plain_text` in the payload.
    ///   2. Image filepaths in `content.paths` (filtered via
    ///      `get_image_filepaths_from_paths`) follow next — Claude
    ///      Code and other image-aware CLIs accept file paths as
    ///      input.
    ///   3. If `content.images` carries any pasteboard image data,
    ///      pick the highest-priority MIME from
    ///      `CLIPBOARD_IMAGE_MIME_TYPES`, save it to
    ///      `$TMPDIR/term_grid_clipboard_<ts>.<ext>`, and append
    ///      the path to the payload.
    ///
    /// Paths are shell-quoted (single-quote escape) so spaces in
    /// names don't break tokenization in the shell.
    ///
    /// The final payload is normalised (CRLF → LF) and wrapped in
    /// `\x1b[200~` … `\x1b[201~` when the emulator has bracketed
    /// paste enabled.
    fn paste_into_focused(&mut self) {
        let content = self.clipboard.read();
        let mut parts: Vec<String> = Vec::new();

        if should_insert_text_on_paste(&content) && !content.plain_text.is_empty() {
            parts.push(content.plain_text.clone());
        }

        if let Some(paths) = content.paths.as_deref() {
            for path in get_image_filepaths_from_paths(paths) {
                parts.push(shell_quote_path(&path));
            }
        }

        if let Some(images) = content.images.as_deref() {
            if let Some(best) = pick_best_image(images) {
                if let Some(path) = save_image_to_temp(best, "term_grid_clipboard") {
                    parts.push(shell_quote_path(&path));
                }
            }
        }

        if parts.is_empty() {
            return;
        }
        let payload = parts.join(" ");

        let id = self.tree.focus();
        let bracketed = self
            .panels
            .get(&id)
            .map(|p| p.emulator.bracketed_paste())
            .unwrap_or(false);
        let bytes = encode_paste(&payload, bracketed);
        if let Some(panel) = self.panels.get_mut(&id) {
            let _ = panel.writer.write_all(&bytes);
            let _ = panel.writer.flush();
        }
    }

    /// Copy the focused panel's selection to the system clipboard.
    /// No-op when nothing's selected or the selection is empty.
    /// Selection is not cleared on copy (modern UX — matches Warp).
    fn copy_focused_selection(&mut self) {
        let id = self.tree.focus();
        let Some(panel) = self.panels.get(&id) else { return };
        let Some(sel) = panel.selection else { return };
        if sel.is_empty() {
            return;
        }
        let snap = panel.emulator.snapshot();
        let text = selection_to_text(&sel, &snap);
        if text.is_empty() {
            return;
        }
        self.clipboard.write(ClipboardContent::plain_text(text));
    }

    /// Snap the focused panel's scroll to either the top (Cmd+Home) or
    /// the bottom (Cmd+End) of its scrollback. Cancels any in-flight
    /// momentum on that panel so the user's jump is not undone.
    ///
    /// Convention: `offset_y == 0` is at the BOTTOM (cursor visible);
    /// `offset_y == max_offset` is at the TOP of the scrollback.
    fn jump_focused_scroll_to(&mut self, bottom: bool) {
        let target = self.tree.focus();
        if !self.panels.contains_key(&target) {
            return;
        }
        if self.scrolling_panel == Some(target) {
            self.cancel_momentum();
            self.cancel_gesture_end();
            self.scroll_velocity = None;
        }
        self.refresh_scroll_geometry(target);
        if let Some(panel) = self.panels.get_mut(&target) {
            panel.scroll.offset_y = if bottom {
                0.0
            } else {
                panel.scroll.max_offset()
            };
        }
    }

    fn on_momentum_tick(&mut self) {
        let Some(target) = self.scrolling_panel else { return };
        let Some(v) = self.scroll_velocity.as_mut() else { return };
        let now = Instant::now();
        let elapsed = now.duration_since(v.last_update).as_secs_f32();
        v.last_update = now;
        v.velocity = decay_velocity(v.velocity, elapsed);
        if v.velocity.length() < MOMENTUM_MIN_VELOCITY {
            self.cancel_momentum();
            self.scroll_velocity = None;
            return;
        }
        let delta = v.velocity * elapsed;
        self.refresh_scroll_geometry(target);
        if let Some(panel) = self.panels.get_mut(&target) {
            panel.scroll.scroll_by(delta.y);
        }
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }

    fn drain_panel(&mut self, id: PanelId) {
        // Follow mode: capture whether the panel was at the bottom of
        // its scrollback BEFORE applying new bytes. If so, re-pin to
        // the bottom afterward so the cursor stays visible while the
        // shell prints. Users who explicitly scrolled up keep position.
        //
        // Our scroll convention (see populate_panel): `offset_y == 0`
        // is at the BOTTOM (visible region with cursor); larger values
        // mean we're looking further into scrollback.
        self.refresh_scroll_geometry(id);
        let was_at_bottom = self
            .panels
            .get(&id)
            .map(|p| p.scroll.offset_y <= SCROLL_BOTTOM_EPSILON)
            .unwrap_or(true);

        let Some(panel) = self.panels.get_mut(&id) else {
            return;
        };
        while let Ok(chunk) = panel.bytes_rx.try_recv() {
            panel.emulator.process(&chunk);
        }
        // Ship the emulator's DA/DSR replies back to the PTY so apps
        // that block on them (less, vim probing terminfo, …) get
        // unstuck.
        let responses = panel.emulator.take_responses();
        if !responses.is_empty() {
            let _ = panel.writer.write_all(&responses);
            let _ = panel.writer.flush();
        }

        if was_at_bottom {
            self.refresh_scroll_geometry(id);
            if let Some(panel) = self.panels.get_mut(&id) {
                panel.scroll.offset_y = 0.0;
            }
            // Cancel any in-flight momentum on this panel — the shell's
            // own output just shifted the viewport, so previous inertia
            // is stale.
            if self.scrolling_panel == Some(id) {
                self.cancel_momentum();
                self.scroll_velocity = None;
            }
        }

        // Text was added — drop any pending selection on this panel.
        // Matches Warp's documented intent
        // (`app/src/terminal/model/selection.rs:1-6`): "cleared when
        // text is added/removed/scrolled on the screen". Don't clear
        // while the user is mid-drag in this panel — they would lose
        // their in-progress gesture.
        if self.dragging_selection != Some(id) {
            if let Some(panel) = self.panels.get_mut(&id) {
                panel.selection = None;
            }
        }
    }

    fn write_to_focused(&mut self, bytes: &[u8]) {
        let focused = self.tree.focus();
        if let Some(panel) = self.panels.get_mut(&focused) {
            let _ = panel.writer.write_all(bytes);
            let _ = panel.writer.flush();
        }
    }

    /// Split the focused panel and spawn a fresh shell into the new
    /// pane. The new shell starts at the default 80×24 grid; the
    /// follow-up `sync_panels_to_tree` resizes both halves to fit
    /// their post-split bounds.
    fn split_focused(&mut self, split: Split) {
        let focused = self.tree.focus();
        let Some(new_id) = self.tree.split(focused, split, 0.5) else {
            return;
        };
        match self.spawn_panel(new_id, INITIAL_GRID_COLS, INITIAL_GRID_ROWS) {
            Ok(state) => {
                self.panels.insert(new_id, state);
            }
            Err(e) => {
                eprintln!("term_grid: failed to spawn shell into new panel: {e}");
                // Roll back the split so the tree stays consistent.
                self.tree.close(new_id);
                return;
            }
        }
        self.sync_panels_to_tree();
    }

    /// Close the focused panel and drop its PTY. Returns `true` if
    /// the demo should exit (no panels remain).
    fn close_focused(&mut self) -> bool {
        let id = self.tree.focus();
        self.panels.remove(&id);
        self.tree.close(id);
        if self.tree.is_empty() {
            true
        } else {
            self.sync_panels_to_tree();
            false
        }
    }

    fn divider_under(&self, x: f32, y: f32) -> Option<Divider> {
        self.tree.dividers().into_iter().find(|d| match d.split {
            Split::Horizontal => {
                x >= d.rect.x
                    && x < d.rect.x + d.rect.w
                    && (y - d.rect.y).abs() <= DIVIDER_HIT_TOLERANCE
            }
            Split::Vertical => {
                y >= d.rect.y
                    && y < d.rect.y + d.rect.h
                    && (x - d.rect.x).abs() <= DIVIDER_HIT_TOLERANCE
            }
        })
    }

    fn on_mouse_press(&mut self) {
        let Some((x, y)) = self.cursor else { return };
        if let Some(d) = self.divider_under(x, y) {
            self.drag = Some(DragState {
                branch: d.id,
                split: d.split,
                bounds: d.bounds,
            });
            return;
        }
        if let Some(id) = self.tree.hit_test(x, y) {
            let focus_changed = self.tree.set_focus(id);
            // Only start a selection when the emulator is NOT in
            // mouse-reporting mode — vim / htop / fzf etc. handle the
            // drag themselves and we mustn't shadow them.
            let owns_mouse = self
                .panels
                .get(&id)
                .map(|p| p.emulator.mouse_mode() != MouseMode::None)
                .unwrap_or(false);
            if !owns_mouse {
                if let Some(point) = self.cell_at_panel(id, x, y) {
                    let count = self.bump_click_count(id, point);
                    // Need snapshot for word / line expansion; take it
                    // before re-borrowing panel mutably.
                    let snap = self
                        .panels
                        .get(&id)
                        .map(|p| p.emulator.snapshot());
                    if let Some(panel) = self.panels.get_mut(&id) {
                        match count {
                            1 => {
                                panel.selection = Some(Selection::new(point));
                                self.dragging_selection = Some(id);
                            }
                            2 => {
                                let (start, end) = snap
                                    .as_ref()
                                    .map(|s| expand_word(point, s))
                                    .unwrap_or((point, point));
                                panel.selection = Some(Selection {
                                    anchor: start,
                                    cursor: end,
                                });
                                // No drag after double-click; user
                                // re-clicks to start a linear selection.
                                self.dragging_selection = None;
                            }
                            _ => {
                                let (start, end) = snap
                                    .as_ref()
                                    .map(|s| expand_line(point, s))
                                    .unwrap_or((point, point));
                                panel.selection = Some(Selection {
                                    anchor: start,
                                    cursor: end,
                                });
                                self.dragging_selection = None;
                            }
                        }
                    }
                }
            }
            if focus_changed {
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
        }
    }

    /// Update `self.last_click` based on the new press and return the
    /// 1..=3 click count. Resets to 1 when the click misses the
    /// previous cell / panel or arrives after the threshold.
    fn bump_click_count(&mut self, panel: PanelId, point: CellPoint) -> u32 {
        let now = Instant::now();
        let new_count = match self.last_click {
            Some(lc)
                if lc.panel == panel
                    && lc.point == point
                    && now.duration_since(lc.time).as_millis() <= MULTI_CLICK_THRESHOLD_MS =>
            {
                if lc.count >= 3 {
                    1
                } else {
                    lc.count + 1
                }
            }
            _ => 1,
        };
        self.last_click = Some(LastClick {
            time: now,
            panel,
            point,
            count: new_count,
        });
        new_count
    }

    fn on_mouse_release(&mut self) {
        if self.drag.take().is_some() {
            // Apply the accumulated divider drag to the PTYs in one
            // shot. Doing this on every cursor move would spam the
            // shell with SIGWINCHes.
            self.sync_panels_to_tree();
            if let Some(w) = self.window.as_ref() {
                w.request_redraw();
            }
        }
        if let Some(id) = self.dragging_selection.take() {
            // A click that didn't drag (anchor == cursor) clears the
            // selection — keeps "click somewhere to deselect" working.
            if let Some(panel) = self.panels.get_mut(&id) {
                if panel.selection.map(|s| s.is_empty()).unwrap_or(false) {
                    panel.selection = None;
                }
            }
        }
    }

    fn on_cursor_moved(&mut self, x: f32, y: f32) {
        self.cursor = Some((x, y));
        if let Some(drag) = self.drag {
            let new_ratio = match drag.split {
                Split::Horizontal => (y - drag.bounds.y) / drag.bounds.h,
                Split::Vertical => (x - drag.bounds.x) / drag.bounds.w,
            };
            if self.tree.drag_divider(drag.branch, new_ratio) {
                // Visual tree updates immediately; the PTY resize is
                // deferred to `on_mouse_release` (see comment there).
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
        }
        if let Some(panel_id) = self.dragging_selection {
            if let Some(point) = self.cell_at_panel(panel_id, x, y) {
                if let Some(panel) = self.panels.get_mut(&panel_id) {
                    if let Some(sel) = panel.selection.as_mut() {
                        sel.cursor = point;
                    }
                }
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
        }
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
            let scroll_offset_y = panel.scroll.offset_y;
            populate_panel(
                &snapshot,
                PanelRect::new(panel_rect.x, panel_rect.y, panel_rect.w, panel_rect.h),
                palette,
                font_system,
                swash_cache,
                renderer.atlas_mut(),
                shape_cache,
                FONT_SIZE,
                sf,
                metrics,
                scroll_offset_y,
                &mut rects,
                &mut glyphs,
            );
            if let Some(sel) = panel.selection {
                push_selection_rects(
                    &sel,
                    &snapshot,
                    PanelRect::new(panel_rect.x, panel_rect.y, panel_rect.w, panel_rect.h),
                    sf,
                    metrics,
                    scroll_offset_y,
                    &mut rects,
                );
            }
            if id == focused {
                if let Some(cr) = build_cursor_rect(
                    snapshot.cursor,
                    snapshot.visible_start(),
                    PanelRect::new(panel_rect.x, panel_rect.y, panel_rect.w, panel_rect.h),
                    sf,
                    metrics,
                    scroll_offset_y,
                ) {
                    rects.push(cr);
                }
                rects.extend(focus_border(panel_rect));
            }
        }

        window.pre_present_notify();
        renderer.render(RenderLayer::rects_and_glyphs(&rects, &glyphs), None, 0.0);
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
        // Resize the initial panel to fit the actual window — winit
        // doesn't fire `Resized` immediately after open on every
        // platform, so an explicit sync here avoids the demo
        // starting at 80×24 inside a much larger window.
        self.sync_panels_to_tree();
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
                self.sync_panels_to_tree();
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale_factor = scale_factor as f32;
                if let Some(r) = self.renderer.as_mut() {
                    r.set_scale_factor(self.scale_factor);
                }
                // Cell metrics depend on scale_factor; invalidate the
                // cache and resync panel grids to the new physical
                // cell size.
                self.cell_metrics = None;
                self.sync_panels_to_tree();
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                // Cmd/Super handles the demo's own shortcuts; Ctrl
                // combos belong to the shell (Ctrl+C / Ctrl+D / ...).
                if self.modifiers.super_key() {
                    // Match on physical_key, not logical_key, so the
                    // shortcuts work on every keyboard layout: a
                    // Russian / French / Greek layout maps `KeyC`
                    // physically to whatever character the OS pleases,
                    // but Cmd+<physical C> should still mean "copy".
                    if let PhysicalKey::Code(code) = event.physical_key {
                        let shift = self.modifiers.shift_key();
                        match code {
                            KeyCode::KeyQ => {
                                event_loop.exit();
                                return;
                            }
                            KeyCode::KeyD => {
                                let split = if shift {
                                    Split::Horizontal
                                } else {
                                    Split::Vertical
                                };
                                self.split_focused(split);
                                if let Some(w) = self.window.as_ref() {
                                    w.request_redraw();
                                }
                                return;
                            }
                            KeyCode::KeyW => {
                                if self.close_focused() {
                                    event_loop.exit();
                                } else if let Some(w) = self.window.as_ref() {
                                    w.request_redraw();
                                }
                                return;
                            }
                            KeyCode::KeyC => {
                                self.copy_focused_selection();
                                return;
                            }
                            KeyCode::KeyV => {
                                self.paste_into_focused();
                                return;
                            }
                            _ => {}
                        }
                    }
                    if let Key::Named(named) = &event.logical_key {
                        match named {
                            NamedKey::Home => {
                                self.jump_focused_scroll_to(false);
                                if let Some(w) = self.window.as_ref() {
                                    w.request_redraw();
                                }
                                return;
                            }
                            NamedKey::End => {
                                self.jump_focused_scroll_to(true);
                                if let Some(w) = self.window.as_ref() {
                                    w.request_redraw();
                                }
                                return;
                            }
                            _ => {}
                        }
                    }
                    // Other Cmd combos: don't forward (Cmd is an app
                    // modifier, not a terminal one).
                    return;
                }
                // Esc clears any active selection on the focused panel
                // (in addition to being forwarded to the shell — vim
                // leaves insert mode, fzf cancels, etc.).
                if matches!(event.logical_key, Key::Named(NamedKey::Escape)) {
                    let focused = self.tree.focus();
                    if let Some(panel) = self.panels.get_mut(&focused) {
                        if panel.selection.take().is_some() {
                            if let Some(w) = self.window.as_ref() {
                                w.request_redraw();
                            }
                        }
                    }
                }
                if let Some(bytes) = encode_key(&event.logical_key, self.modifiers) {
                    self.write_to_focused(&bytes);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let PhysicalPosition { x, y } = position;
                self.on_cursor_moved(x as f32 / self.scale_factor, y as f32 / self.scale_factor);
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => match state {
                ElementState::Pressed => self.on_mouse_press(),
                ElementState::Released => self.on_mouse_release(),
            },
            WindowEvent::MouseWheel { delta, phase, .. } => {
                let (precise, dy) = match delta {
                    MouseScrollDelta::PixelDelta(p) => (true, p.y as f32),
                    MouseScrollDelta::LineDelta(_, v) => (false, v * NUM_PIXELS_PER_LINE),
                };
                self.on_wheel(dy, phase, precise);
            }
            WindowEvent::RedrawRequested => self.on_redraw(),
            _ => {}
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: CustomEvent) {
        match event {
            CustomEvent::BytesArrived(id) => {
                self.drain_panel(id);
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            CustomEvent::PanelExited(id) => {
                if self.scrolling_panel == Some(id) {
                    self.cancel_momentum();
                    self.cancel_gesture_end();
                    self.scrolling_panel = None;
                    self.scroll_velocity = None;
                }
                self.panels.remove(&id);
                self.tree.close(id);
                if self.tree.is_empty() {
                    event_loop.exit();
                } else {
                    // Sibling absorbed the closed panel's bounds —
                    // resize its grid to match.
                    self.sync_panels_to_tree();
                    if let Some(w) = self.window.as_ref() {
                        w.request_redraw();
                    }
                }
            }
            CustomEvent::GestureEnded(id) => {
                if self.scrolling_panel == Some(id) {
                    self.on_gesture_end();
                }
            }
            CustomEvent::MomentumTick(id) => {
                if self.scrolling_panel == Some(id) {
                    self.on_momentum_tick();
                }
            }
        }
    }
}

/// Spawn a one-shot abortable timer that sends `event` after `delay`.
/// Mirrors `scroll_demo::schedule_once`.
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

/// Spawn an abortable loop that sends `MomentumTick(panel_id)` every
/// `interval` until aborted or the receiver is gone.
fn schedule_momentum_loop(
    proxy: EventLoopProxy<CustomEvent>,
    interval: std::time::Duration,
    panel: PanelId,
) -> AbortHandle {
    let (fut, abort) = abortable(async move {
        loop {
            Delay::new(interval).await;
            if proxy.send_event(CustomEvent::MomentumTick(panel)).is_err() {
                break;
            }
        }
    });
    std::thread::spawn(move || {
        let _ = futures::executor::block_on(fut);
    });
    abort
}


/// Construct the platform clipboard. macOS gets `MacClipboard`;
/// other platforms fall back to `InMemoryClipboard` (the demo is
/// macOS-targeted today, but the type already abstracts this).
fn make_clipboard() -> Box<dyn Clipboard> {
    #[cfg(target_os = "macos")]
    {
        Box::new(term_clipboard::MacClipboard::new())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(term_clipboard::InMemoryClipboard::default())
    }
}


fn focus_border(rect: Rect) -> [RectInstance; 4] {
    let b = FOCUS_BORDER;
    [
        RectInstance {
            pos: [rect.x, rect.y],
            size: [rect.w, b],
            color: FOCUS_COLOR,
        },
        RectInstance {
            pos: [rect.x, rect.y + rect.h - b],
            size: [rect.w, b],
            color: FOCUS_COLOR,
        },
        RectInstance {
            pos: [rect.x, rect.y],
            size: [b, rect.h],
            color: FOCUS_COLOR,
        },
        RectInstance {
            pos: [rect.x + rect.w - b, rect.y],
            size: [b, rect.h],
            color: FOCUS_COLOR,
        },
    ]
}



fn main() {
    let event_loop = EventLoop::<CustomEvent>::with_user_event()
        .build()
        .expect("failed to create event loop");
    let proxy = event_loop.create_proxy();
    let mut app = App::new(proxy);
    event_loop.run_app(&mut app).expect("event loop failed");
}
