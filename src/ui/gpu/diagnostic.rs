//! Cmd+Shift+D snapshot dump for the GPU UI.
//!
//! Prints grid dimensions, scroll offset, cursor state, visible-row
//! range, title, and the first four visible rows (chars + non-zero
//! attribute flags) to stderr. Used to capture the emulator's
//! perspective at the moment a user-visible rendering bug surfaces.
//! See `feedback_capture_pty_bytes_for_render_bugs` — this is the
//! companion to the `ANYCLAUDE_DEBUG_PTY` byte tee in `pty.rs`.

use term_core::RenderSnapshot;

/// Dump the diagnostic snapshot to stderr. Free function so the
/// caller (`GpuApp::on_diagnostic_key`) can pass borrowed pieces of
/// itself without holding `&self` across the whole call.
pub(super) fn dump_snapshot(
    grid_size: (usize, usize),
    scroll_offset_y: f32,
    scroll_max_offset: f32,
    snapshot: Option<&RenderSnapshot>,
) {
    eprintln!("=== anyclaude diagnostic snapshot ===");
    eprintln!("grid_size: {} cols x {} rows", grid_size.0, grid_size.1);
    eprintln!(
        "scroll: offset_y={:.2}, max={:.2}",
        scroll_offset_y, scroll_max_offset
    );
    let Some(snap) = snapshot else {
        eprintln!("(no emulator)");
        eprintln!("=== end snapshot ===");
        return;
    };
    eprintln!(
        "cursor: row={}, col={}, visible={}, style={:?}",
        snap.cursor.row, snap.cursor.col, snap.cursor.visible, snap.cursor.style
    );
    eprintln!(
        "visible_rows: {}, total_rows: {}, visible_start: {}",
        snap.visible_rows,
        snap.rows.len(),
        snap.visible_start()
    );
    eprintln!("title: {:?}", snap.title);
    for (offset, row) in snap.visible_iter().take(4).enumerate() {
        let chars: String = row.cells.iter().take(60).map(|c| c.c).collect();
        eprintln!("row[{offset:02}]: {chars:?}");
        for (i, c) in row.cells.iter().enumerate().take(60) {
            if c.flags.bits() != 0 {
                eprintln!(
                    "    [{offset:02}][{i:02}]={:?} flags=0x{:04x}",
                    c.c,
                    c.flags.bits()
                );
            }
        }
    }
    eprintln!("=== end snapshot ===");
}
