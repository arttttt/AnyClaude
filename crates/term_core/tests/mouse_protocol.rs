//! Mouse-protocol DECSET handling (§6). The tracking level (1000 / 1002 /
//! 1003) and the encoding (1006) are orthogonal — the regression these pin is
//! the old single-enum model where enabling SGR clobbered the tracking level
//! (and vice versa).

use term_core::{MouseEncoding, MouseTracking, TerminalEmulator, VtEmulator};

fn emu() -> VtEmulator {
    VtEmulator::new(80, 24, 0)
}

#[test]
fn default_protocol_is_off_and_legacy() {
    let p = emu().mouse_protocol();
    assert_eq!(p.tracking, MouseTracking::Off);
    assert_eq!(p.encoding, MouseEncoding::Default);
    assert!(!p.is_active());
}

#[test]
fn tracking_and_encoding_compose_without_clobbering() {
    // 1000 (click tracking) then 1006 (SGR) — both must end up set. This is the
    // exact case the conflated enum broke.
    let mut e = emu();
    e.process(b"\x1b[?1000h");
    e.process(b"\x1b[?1006h");
    let p = e.mouse_protocol();
    assert_eq!(p.tracking, MouseTracking::Normal);
    assert_eq!(p.encoding, MouseEncoding::Sgr);
    assert!(p.is_active());
}

#[test]
fn compose_is_order_independent() {
    // SGR first, then tracking — same result.
    let mut e = emu();
    e.process(b"\x1b[?1006h");
    e.process(b"\x1b[?1002h");
    let p = e.mouse_protocol();
    assert_eq!(p.tracking, MouseTracking::ButtonEvent);
    assert_eq!(p.encoding, MouseEncoding::Sgr);
    assert!(p.reports_motion());
    assert!(!p.reports_bare_motion());
}

#[test]
fn any_event_reports_bare_motion() {
    let mut e = emu();
    e.process(b"\x1b[?1003h");
    let p = e.mouse_protocol();
    assert_eq!(p.tracking, MouseTracking::AnyEvent);
    assert!(p.reports_motion());
    assert!(p.reports_bare_motion());
}

#[test]
fn disabling_tracking_leaves_encoding_intact() {
    let mut e = emu();
    e.process(b"\x1b[?1000h");
    e.process(b"\x1b[?1006h");
    e.process(b"\x1b[?1000l"); // turn click tracking back off
    let p = e.mouse_protocol();
    assert_eq!(p.tracking, MouseTracking::Off);
    assert_eq!(p.encoding, MouseEncoding::Sgr, "encoding must survive");
}

#[test]
fn disabling_sgr_leaves_tracking_intact() {
    let mut e = emu();
    e.process(b"\x1b[?1002h");
    e.process(b"\x1b[?1006h");
    e.process(b"\x1b[?1006l"); // back to legacy encoding
    let p = e.mouse_protocol();
    assert_eq!(p.tracking, MouseTracking::ButtonEvent);
    assert_eq!(p.encoding, MouseEncoding::Default);
}

#[test]
fn resetting_a_nonactive_level_is_a_noop() {
    // current = AnyEvent (1003); a 1002 reset must NOT downgrade it.
    let mut e = emu();
    e.process(b"\x1b[?1003h");
    e.process(b"\x1b[?1002l");
    assert_eq!(e.mouse_protocol().tracking, MouseTracking::AnyEvent);
}
