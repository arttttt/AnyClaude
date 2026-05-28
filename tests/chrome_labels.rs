//! The chrome presenter (`ui::chrome_labels`) — pure, GPU-free formatting, so
//! it is tested directly here. Pins the segment text/colour/order against the
//! live header/footer so the term_ui port can't silently drift from it.

use anyclaude::ui::chrome_labels::{
    footer_segments, header_segments, CHROME_FLASH_COLOR, CHROME_TEXT_COLOR,
};

#[test]
fn session_copied_flips_text_and_color() {
    let segs = header_segments("anthropic", Some("opus"), None, 5, 42, "abc123", true);
    let session = segs.last().expect("session segment present");
    assert_eq!(session.text, "Session ID copied!");
    assert_eq!(session.color, CHROME_FLASH_COLOR);
}

#[test]
fn session_idle_shows_id_in_dim() {
    let segs = header_segments("anthropic", Some("opus"), Some("mate"), 5, 42, "abc123", false);
    let session = segs.last().expect("session segment present");
    assert_eq!(session.text, "Session: abc123");
    assert_eq!(session.color, CHROME_TEXT_COLOR);
}

#[test]
fn absent_subagent_and_teammate_render_em_dash() {
    let segs = header_segments("anthropic", None, None, 0, 0, "s", false);
    assert_eq!(segs[1].text, "sub: —");
    assert_eq!(segs[2].text, "team: —");
}

#[test]
fn header_has_six_segments_in_order() {
    let segs = header_segments("backend-x", Some("a"), Some("b"), 7, 13, "sid", false);
    assert_eq!(segs.len(), 6);
    assert_eq!(segs[0].text, "backend: backend-x");
    assert_eq!(segs[1].text, "sub: a");
    assert_eq!(segs[2].text, "team: b");
    assert_eq!(segs[3].text, "Reqs: 7");
    assert_eq!(segs[4].text, "Uptime: 13s");
}

#[test]
fn footer_hints_left_version_right() {
    let (left, right) = footer_segments("0.5.0");
    assert_eq!(left.len(), 1);
    assert!(left[0].text.contains("Cmd+B: Switch"));
    assert!(left[0].text.contains("Cmd+Q: Quit"));
    assert_eq!(right.len(), 1);
    assert_eq!(right[0].text, "v0.5.0 ");
}
