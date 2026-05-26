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
use term_core::{
    create_emulator, AnsiPalette, CellFlags, CursorState, CursorStyle, MouseMode, RenderSnapshot,
    TermColor, TerminalEmulator,
};
use term_gpu::{
    decay_velocity, rasterize_glyph, FontFamily, GlyphAtlas, GlyphInstance, GpuRenderer,
    RectInstance, ScrollState, ScrollVelocity, Style, TextShapeCache, Weight, GESTURE_END_TIMEOUT,
    MOMENTUM_FRAME_INTERVAL, MOMENTUM_MIN_VELOCITY, MOMENTUM_THRESHOLD, NUM_PIXELS_PER_LINE,
};
use term_layout::{BranchId, Divider, PanelId, PanelTree, Rect, Split};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
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
/// Highlight colour for selected cells. Matches Warp's
/// `text_selection_color` at
/// `crates/warp_core/src/ui/theme/color.rs:300` —
/// `rgba(118, 167, 250, 0.4)`.
const SELECTION_COLOR: [f32; 4] = [118.0 / 255.0, 167.0 / 255.0, 250.0 / 255.0, 0.4];

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

/// Cell coordinates in absolute-row form: `row` indexes into
/// `RenderSnapshot::rows` (scrollback first, then visible), so the
/// selection stays anchored to its content as the viewport scrolls.
/// `col` is in cells [0, cols).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CellPoint {
    row: usize,
    col: usize,
}

impl PartialOrd for CellPoint {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CellPoint {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.row, self.col).cmp(&(other.row, other.col))
    }
}

/// Linear text selection inside a single panel. Modelled on Warp's
/// `app/src/terminal/model/selection.rs` doc-comment:
/// "A selection should start when the mouse is clicked, finalized
///  when the button is released, cleared when text is added/removed
///  /scrolled on the screen, and cleared if the user clicks off".
#[derive(Debug, Clone, Copy)]
struct Selection {
    /// Where the mouse first pressed down.
    anchor: CellPoint,
    /// Where the mouse currently is (or was released).
    cursor: CellPoint,
}

impl Selection {
    fn new(point: CellPoint) -> Self {
        Self {
            anchor: point,
            cursor: point,
        }
    }

    fn is_empty(&self) -> bool {
        self.anchor == self.cursor
    }

    /// Returns the normalized `(start, end)` range with `start <= end`
    /// in document order.
    fn range(&self) -> (CellPoint, CellPoint) {
        if self.anchor <= self.cursor {
            (self.anchor, self.cursor)
        } else {
            (self.cursor, self.anchor)
        }
    }
}

#[derive(Clone, Copy)]
struct CellMetrics {
    width_physical: f32,
    height_physical: f32,
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
            cursor: None,
            drag: None,
            scrolling_panel: None,
            scroll_velocity: None,
            momentum_abort: None,
            gesture_end_abort: None,
            dragging_selection: None,
            proxy,
        }
    }

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
                    if let Some(panel) = self.panels.get_mut(&id) {
                        panel.selection = Some(Selection::new(point));
                    }
                    self.dragging_selection = Some(id);
                }
            }
            if focus_changed {
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
        }
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
                panel_rect,
                palette,
                font_system,
                swash_cache,
                renderer.atlas_mut(),
                shape_cache,
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
                    panel_rect,
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
                    panel_rect,
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
                    if let Key::Character(c) = &event.logical_key {
                        let shift = self.modifiers.shift_key();
                        match c.as_str() {
                            "q" | "Q" => {
                                event_loop.exit();
                                return;
                            }
                            "d" | "D" => {
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
                            "w" | "W" => {
                                if self.close_focused() {
                                    event_loop.exit();
                                } else if let Some(w) = self.window.as_ref() {
                                    w.request_redraw();
                                }
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
    scroll_offset_y_logical: f32,
    rects: &mut Vec<RectInstance>,
    glyphs: &mut Vec<GlyphInstance>,
) {
    let sf = scale_factor;
    let cell_w_logical = metrics.width_physical / sf;
    let cell_h_logical = metrics.height_physical / sf;
    let panel_origin_x_physical = panel_rect.x * sf;
    let panel_origin_y_physical = panel_rect.y * sf;
    let scroll_offset_y_physical = scroll_offset_y_logical * sf;
    let total_rows = snapshot.rows.len();
    let visible_rows = snapshot.visible_rows;
    // The visible region of the buffer is anchored at the BOTTOM of the
    // panel — i.e. with `scroll_offset_y = 0` (no scrollback shown) row
    // `total - visible` should land at the panel's first cell. We
    // accomplish this by shifting every row up by
    // `(total - visible) * cell_h`, then applying the scroll offset.
    let baseline_offset_phys = (total_rows.saturating_sub(visible_rows)) as f32
        * metrics.height_physical;
    let mut cell_text = String::with_capacity(8);

    // Clip the cell grid to the panel's logical bounds. While a
    // drag is in flight the PanelTree's rect updates immediately but
    // the emulator (and its `rows.len()` / `cells.len()`) is still
    // sized to the pre-drag bounds; without this cull, glyphs from
    // the larger grid spill into the neighbouring panel.
    let panel_max_x_phys = panel_rect.w * sf;
    let panel_max_y_phys = panel_rect.h * sf;
    for (row_idx, row) in snapshot.rows.iter().enumerate() {
        // Y of this row's top edge relative to panel top, in physical px.
        // `+ scroll_offset` because scrolling UP visually moves rows DOWN.
        let row_y_phys = row_idx as f32 * metrics.height_physical
            - baseline_offset_phys
            + scroll_offset_y_physical;
        if row_y_phys + metrics.height_physical <= 0.0 || row_y_phys >= panel_max_y_phys {
            continue;
        }
        for (col_idx, cell) in row.cells.iter().enumerate() {
            let col_x_phys = col_idx as f32 * metrics.width_physical;
            if col_x_phys >= panel_max_x_phys {
                break;
            }
            let inverse = cell.flags.contains(CellFlags::INVERSE);
            let (fg_eff, bg_eff) = if inverse {
                (cell.bg, cell.fg)
            } else {
                (cell.fg, cell.bg)
            };

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
            let has_decoration = cell.flags.underline()
                || cell.flags.double_underline()
                || cell.flags.strike();
            // Nothing to render: blank cell with no fg, no decorations.
            if is_blank && fg_eff == TermColor::Default && !has_decoration {
                continue;
            }

            let mut color = if fg_eff == TermColor::Default {
                DEFAULT_FG
            } else {
                fg_eff.to_rgba(palette)
            };
            if cell.flags.faint() {
                color[3] *= 0.5;
            }

            // SGR HIDDEN suppresses the glyph but keeps bg and any
            // decoration lines (matches xterm/iTerm behavior).
            let push_glyph = !cell.flags.hidden() && !is_blank;
            if push_glyph {
                cell_text.clear();
                cell_text.push(cell.c);
                if let Some(extra) = &cell.extra {
                    for c in &extra.zerowidth {
                        cell_text.push(*c);
                    }
                }

                let cell_origin_x_phys = panel_origin_x_physical + col_x_phys;
                let cell_origin_y_phys = panel_origin_y_physical + row_y_phys;

                let weight = if cell.flags.bold() {
                    Weight::BOLD
                } else {
                    Weight::NORMAL
                };
                let style = if cell.flags.italic() {
                    Style::Italic
                } else {
                    Style::Normal
                };
                let shaped = shape_cache.shape(
                    font_system,
                    &cell_text,
                    FONT_SIZE,
                    sf,
                    None,
                    weight,
                    style,
                );
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

            // SGR decoration lines. Positions are vertical fractions of
            // the cell height: underline sits just below the baseline
            // (~0.78), strike crosses the x-height midline (~0.42), the
            // double-underline pair brackets the regular underline
            // position.
            if cell.flags.underline() {
                rects.push(RectInstance {
                    pos: [pos_x_logical, pos_y_logical + cell_h_logical * 0.78],
                    size: [cell_w_logical, 1.0],
                    color,
                });
            }
            if cell.flags.double_underline() {
                rects.push(RectInstance {
                    pos: [pos_x_logical, pos_y_logical + cell_h_logical * 0.72],
                    size: [cell_w_logical, 0.8],
                    color,
                });
                rects.push(RectInstance {
                    pos: [pos_x_logical, pos_y_logical + cell_h_logical * 0.84],
                    size: [cell_w_logical, 0.8],
                    color,
                });
            }
            if cell.flags.strike() {
                rects.push(RectInstance {
                    pos: [pos_x_logical, pos_y_logical + cell_h_logical * 0.42],
                    size: [cell_w_logical, 1.0],
                    color,
                });
            }
        }
    }
}

/// Push a `RectInstance` for every cell inside the selection range,
/// using the same row positioning math as `populate_panel`. Selection
/// rects render after background rects (so they tint the bg) but
/// before glyphs (so text stays legible through the highlight).
///
/// Wide-span selections produce one rect per cell — a future
/// optimisation could coalesce adjacent cells into runs, but at our
/// cell counts (panel ≤ ~250×80 ≈ 20k cells max, typical selection
/// well under 1k) this is below the noise floor.
fn push_selection_rects(
    sel: &Selection,
    snapshot: &RenderSnapshot,
    panel_rect: Rect,
    scale_factor: f32,
    metrics: CellMetrics,
    scroll_offset_y_logical: f32,
    rects: &mut Vec<RectInstance>,
) {
    let (start, end) = sel.range();
    if start == end {
        return;
    }
    let sf = scale_factor;
    let cell_h_logical = metrics.height_physical / sf;
    let panel_origin_x_physical = panel_rect.x * sf;
    let panel_origin_y_physical = panel_rect.y * sf;
    let scroll_offset_y_physical = scroll_offset_y_logical * sf;
    let total_rows = snapshot.rows.len();
    let visible_rows = snapshot.visible_rows;
    let baseline_offset_phys =
        total_rows.saturating_sub(visible_rows) as f32 * metrics.height_physical;
    let panel_max_x_phys = panel_rect.w * sf;
    let panel_max_y_phys = panel_rect.h * sf;
    let end_row = end.row.min(total_rows.saturating_sub(1));
    for row_idx in start.row..=end_row {
        let Some(row) = snapshot.rows.get(row_idx) else {
            continue;
        };
        let row_y_phys = row_idx as f32 * metrics.height_physical
            - baseline_offset_phys
            + scroll_offset_y_physical;
        if row_y_phys + metrics.height_physical <= 0.0 || row_y_phys >= panel_max_y_phys {
            continue;
        }
        let cols = row.cells.len();
        // Linear (row-wrapping) selection: full row on intermediate
        // lines; clipped on first / last lines by start.col / end.col.
        let col_start = if row_idx == start.row { start.col } else { 0 };
        let col_end = if row_idx == end.row { end.col } else { cols };
        if col_start >= col_end {
            continue;
        }
        let span_cells = col_end - col_start;
        let span_w_phys = (span_cells as f32 * metrics.width_physical)
            .min(panel_max_x_phys - col_start as f32 * metrics.width_physical);
        if span_w_phys <= 0.0 {
            continue;
        }
        let pos_x_logical =
            (panel_origin_x_physical + col_start as f32 * metrics.width_physical) / sf;
        let pos_y_logical = (panel_origin_y_physical + row_y_phys) / sf;
        rects.push(RectInstance {
            pos: [pos_x_logical, pos_y_logical],
            size: [span_w_phys / sf, cell_h_logical],
            color: SELECTION_COLOR,
        });
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

fn build_cursor_rect(
    cursor: CursorState,
    visible_start: usize,
    panel_rect: Rect,
    scale_factor: f32,
    metrics: CellMetrics,
    scroll_offset_y_logical: f32,
) -> Option<RectInstance> {
    if !cursor.visible {
        return None;
    }
    let sf = scale_factor;
    let cell_offset_x_phys = cursor.col as f32 * metrics.width_physical;
    // Cursor's row is visible-relative; combine with `visible_start` to
    // get the absolute row, then subtract the visible-anchor offset so
    // that `scroll_offset_y == 0` puts the cursor at its expected place
    // inside the panel.
    let abs_row = visible_start + cursor.row as usize;
    let scroll_offset_y_phys = scroll_offset_y_logical * sf;
    let baseline_offset_phys =
        visible_start as f32 * metrics.height_physical;
    let cell_offset_y_phys = abs_row as f32 * metrics.height_physical
        - baseline_offset_phys
        + scroll_offset_y_phys;
    // Cull when the cursor's cell origin lies outside the panel's
    // logical bounds — during a divider drag the PanelTree shrinks
    // before the emulator gets SIGWINCH, AND when the user has
    // scrolled up into history the cursor falls past the bottom of
    // the visible region.
    let panel_h_phys = panel_rect.h * sf;
    if cell_offset_x_phys >= panel_rect.w * sf
        || cell_offset_y_phys + metrics.height_physical <= 0.0
        || cell_offset_y_phys >= panel_h_phys
    {
        return None;
    }
    let cell_x_phys = panel_rect.x * sf + cell_offset_x_phys;
    let cell_y_phys = panel_rect.y * sf + cell_offset_y_phys;
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

/// Encode a winit key event into the byte sequence a typical
/// terminal sends to the PTY. Covers printable text, named keys
/// (Enter / Tab / arrows / etc.), `Ctrl+letter` control codes, and
/// `Alt+key` as ESC-prefixed Meta. Returns `None` when the key has
/// no terminal-byte equivalent (modifier keys alone, function keys
/// we don't translate, IME composition events, …).
fn encode_key(key: &Key, modifiers: ModifiersState) -> Option<Vec<u8>> {
    let ctrl = modifiers.control_key();
    let alt = modifiers.alt_key();
    match key {
        Key::Character(s) => {
            let chars: Vec<char> = s.chars().collect();
            if ctrl && chars.len() == 1 {
                let ch = chars[0];
                if ch.is_ascii_alphabetic() {
                    // Ctrl+A..Z → 0x01..0x1A.
                    return Some(vec![(ch.to_ascii_lowercase() as u8) - b'a' + 1]);
                }
                // A few non-letter Ctrl combos shells expect to receive.
                let mapped = match ch {
                    '[' => Some(0x1b),
                    '\\' => Some(0x1c),
                    ']' => Some(0x1d),
                    '~' | '^' => Some(0x1e),
                    '?' | '/' => Some(0x1f),
                    ' ' => Some(0x00),
                    _ => None,
                };
                if let Some(b) = mapped {
                    return Some(vec![b]);
                }
            }
            let mut bytes = s.as_str().as_bytes().to_vec();
            if alt {
                // ESC-prefix is the conventional encoding for Meta+key.
                bytes.insert(0, 0x1b);
            }
            Some(bytes)
        }
        Key::Named(named) => match named {
            NamedKey::Enter => Some(b"\r".to_vec()),
            NamedKey::Tab => Some(b"\t".to_vec()),
            NamedKey::Backspace => Some(b"\x7f".to_vec()),
            NamedKey::Escape => Some(b"\x1b".to_vec()),
            NamedKey::Space => Some(b" ".to_vec()),
            NamedKey::ArrowUp => Some(b"\x1b[A".to_vec()),
            NamedKey::ArrowDown => Some(b"\x1b[B".to_vec()),
            NamedKey::ArrowRight => Some(b"\x1b[C".to_vec()),
            NamedKey::ArrowLeft => Some(b"\x1b[D".to_vec()),
            NamedKey::Home => Some(b"\x1b[H".to_vec()),
            NamedKey::End => Some(b"\x1b[F".to_vec()),
            NamedKey::Delete => Some(b"\x1b[3~".to_vec()),
            NamedKey::PageUp => Some(b"\x1b[5~".to_vec()),
            NamedKey::PageDown => Some(b"\x1b[6~".to_vec()),
            _ => None,
        },
        _ => None,
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
