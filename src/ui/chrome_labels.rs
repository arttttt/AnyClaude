//! Domain presenter for the GPU chrome: maps anyclaude's runtime facts
//! (active backend, subagent / teammate, request count, uptime, session id, and
//! the "copied!" flash) into the pre-formatted [`uikit::Segment`]s that
//! `uikit::header_bar` / `uikit::footer_bar` lay out.
//!
//! This is the layer that owns the words — "backend:/sub:/team:/Reqs:/Uptime:/
//! Session:" and the flash colour live HERE, so `uikit` can stay
//! domain-agnostic. It takes primitives only (no `BackendState`, no PTY), so it
//! is a pure, GPU-free function that the integration tests exercise directly.
//!
//! The text mirrors the live `ui::gpu::chrome::{draw_header, draw_footer}`
//! exactly; the two converge when the legacy chrome is deleted at cutover
//! (Phase E/F), at which point these constants fold into a single palette.

use uikit::Segment;

/// Dim grey for chrome labels and the inter-segment separator.
// TODO(cutover): unify with `ui::gpu::chrome::CHROME_TEXT_COLOR` (Phase E/F).
pub const CHROME_TEXT_COLOR: [f32; 4] = [0.55, 0.55, 0.55, 1.0];

/// Green flash for the "Session ID copied!" confirmation.
// TODO(cutover): unify with `ui::gpu::chrome::CHROME_FLASH_COLOR` (Phase E/F).
pub const CHROME_FLASH_COLOR: [f32; 4] = [0.4, 0.85, 0.4, 1.0];

/// 1px fence between chrome and the terminal panel.
// TODO(cutover): unify with `ui::gpu::chrome::CHROME_SEPARATOR_COLOR` (Phase E/F).
pub const CHROME_SEPARATOR_COLOR: [f32; 4] = [0.25, 0.25, 0.27, 1.0];

/// Separator drawn between header segments.
pub const HEADER_SEPARATOR: &str = " │ ";

/// Footer hotkey hints (flush-left). Mirrors the live footer verbatim.
pub const FOOTER_HINTS: &str =
    " Cmd+B: Switch │ Cmd+H: History │ Cmd+E: Settings │ Cmd+R: Restart │ Cmd+Q: Quit";

/// Build the header segments in order: backend / sub / team / Reqs / Uptime /
/// Session. `subagent` / `teammate` render as "—" when absent. The Session run
/// flips to `CHROME_FLASH_COLOR` + "Session ID copied!" while `session_copied`
/// holds; otherwise it shows the dim "Session: {id}".
pub fn header_segments(
    active_backend: &str,
    subagent: Option<&str>,
    teammate: Option<&str>,
    reqs: u64,
    uptime_secs: u64,
    session_id: &str,
    session_copied: bool,
) -> Vec<Segment> {
    let sub = subagent.unwrap_or("—");
    let team = teammate.unwrap_or("—");
    let mut segs = vec![
        Segment::new(format!("backend: {active_backend}"), CHROME_TEXT_COLOR),
        Segment::new(format!("sub: {sub}"), CHROME_TEXT_COLOR),
        Segment::new(format!("team: {team}"), CHROME_TEXT_COLOR),
        Segment::new(format!("Reqs: {reqs}"), CHROME_TEXT_COLOR),
        Segment::new(format!("Uptime: {uptime_secs}s"), CHROME_TEXT_COLOR),
    ];
    let session = if session_copied {
        Segment::new("Session ID copied!", CHROME_FLASH_COLOR)
    } else {
        Segment::new(format!("Session: {session_id}"), CHROME_TEXT_COLOR)
    };
    segs.push(session);
    segs
}

/// Build the footer segments as `(left, right)`: the hotkey hints flush-left,
/// the version string flush-right. `version` is the binary's own
/// `CARGO_PKG_VERSION` (passed in — a uikit/presenter `env!` would read the
/// wrong crate's version).
pub fn footer_segments(version: &str) -> (Vec<Segment>, Vec<Segment>) {
    let left = vec![Segment::new(FOOTER_HINTS, CHROME_TEXT_COLOR)];
    let right = vec![Segment::new(format!("v{version} "), CHROME_TEXT_COLOR)];
    (left, right)
}
