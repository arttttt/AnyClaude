//! Mouse-event encoders (§6 mouse reporting). The bytes are spec-defined
//! (xterm), so these pin the exact output — the one reliable check, since the
//! feature has no consumer to verify against live in a keyboard-driven app.

use term_core::{MouseEncoding, MouseProtocol, MouseTracking};
use term_gpu::{
    encode_motion_report, encode_mouse_report, encode_mouse_sgr, encode_mouse_x10, MouseButton,
    MouseEventKind,
};

fn proto(tracking: MouseTracking, encoding: MouseEncoding) -> MouseProtocol {
    MouseProtocol { tracking, encoding }
}

#[test]
fn x10_left_press_offsets_every_field_by_32() {
    // left button (0), col 1, row 1 → ESC [ M  <32+0> <32+1> <32+1>.
    assert_eq!(encode_mouse_x10(0, 1, 1), vec![0x1b, b'[', b'M', 32, 33, 33]);
}

#[test]
fn x10_release_is_button_three() {
    assert_eq!(encode_mouse_x10(3, 5, 10), vec![0x1b, b'[', b'M', 35, 37, 42]);
}

#[test]
fn x10_wheel_up_and_down_codes() {
    assert_eq!(encode_mouse_x10(64, 1, 1)[3], 32 + 64);
    assert_eq!(encode_mouse_x10(65, 1, 1)[3], 32 + 65);
}

#[test]
fn x10_clamps_coordinates_at_223() {
    let bytes = encode_mouse_x10(0, 300, 300);
    assert_eq!(bytes[4], 255, "32 + 223");
    assert_eq!(bytes[5], 255);
}

#[test]
fn sgr_press_uses_capital_m() {
    assert_eq!(encode_mouse_sgr(0, 12, 34, true), b"\x1b[<0;12;34M".to_vec());
}

#[test]
fn sgr_release_uses_lowercase_m_with_the_same_button() {
    assert_eq!(encode_mouse_sgr(0, 12, 34, false), b"\x1b[<0;12;34m".to_vec());
}

#[test]
fn sgr_wheel_has_no_coordinate_limit() {
    assert_eq!(encode_mouse_sgr(64, 500, 999, true), b"\x1b[<64;500;999M".to_vec());
}

// --- encode_mouse_report (the semantic composer over the two formatters) ---

#[test]
fn report_sgr_middle_and_right_carry_button_one_and_two() {
    // Middle = button 1, right = button 2 (SGR keeps the real code).
    assert_eq!(
        encode_mouse_report(MouseButton::Middle, MouseEventKind::Press, 3, 4, true),
        b"\x1b[<1;3;4M".to_vec()
    );
    assert_eq!(
        encode_mouse_report(MouseButton::Right, MouseEventKind::Press, 3, 4, true),
        b"\x1b[<2;3;4M".to_vec()
    );
}

#[test]
fn report_sgr_release_keeps_button_and_uses_lowercase_m() {
    // A right release reports button 2 with the trailing 'm' (button kept).
    assert_eq!(
        encode_mouse_report(MouseButton::Right, MouseEventKind::Release, 7, 8, true),
        b"\x1b[<2;7;8m".to_vec()
    );
}

#[test]
fn report_sgr_motion_sets_the_plus_32_bit() {
    // Left drag = button 0 + 32 motion bit = 32, press-form 'M'.
    assert_eq!(
        encode_mouse_report(MouseButton::Left, MouseEventKind::Motion, 2, 2, true),
        b"\x1b[<32;2;2M".to_vec()
    );
    // Right drag = 2 + 32 = 34.
    assert_eq!(
        encode_mouse_report(MouseButton::Right, MouseEventKind::Motion, 2, 2, true),
        b"\x1b[<34;2;2M".to_vec()
    );
}

#[test]
fn report_legacy_press_and_release_button_bits() {
    // Legacy middle press = button 1 → 32+1 in the button byte.
    assert_eq!(
        encode_mouse_report(MouseButton::Middle, MouseEventKind::Press, 1, 1, false),
        vec![0x1b, b'[', b'M', 33, 33, 33]
    );
    // Legacy release carries no button identity → button-bits 3.
    assert_eq!(
        encode_mouse_report(MouseButton::Right, MouseEventKind::Release, 1, 1, false),
        vec![0x1b, b'[', b'M', 35, 33, 33]
    );
}

#[test]
fn report_legacy_motion_sets_plus_32_in_the_button_byte() {
    // Left drag legacy = 0 + 32 motion = 32 → byte 32+32 = 64.
    assert_eq!(
        encode_mouse_report(MouseButton::Left, MouseEventKind::Motion, 1, 1, false)[3],
        64
    );
}

#[test]
fn report_bare_motion_uses_no_button_code_three() {
    // Any-event (1003) motion with no button held = code 3 + 32 motion = 35.
    assert_eq!(
        encode_mouse_report(MouseButton::None, MouseEventKind::Motion, 9, 9, true),
        b"\x1b[<35;9;9M".to_vec()
    );
    // Legacy form: 35 → button byte 32+35 = 67.
    assert_eq!(
        encode_mouse_report(MouseButton::None, MouseEventKind::Motion, 9, 9, false)[3],
        67
    );
}

#[test]
fn report_wheel_maps_to_64_and_65_in_both_encodings() {
    assert_eq!(
        encode_mouse_report(MouseButton::WheelUp, MouseEventKind::Press, 1, 1, true),
        b"\x1b[<64;1;1M".to_vec()
    );
    assert_eq!(
        encode_mouse_report(MouseButton::WheelDown, MouseEventKind::Press, 1, 1, false)[3],
        32 + 65
    );
}

// --- encode_motion_report (the tracking-level / dedup gating) ---

#[test]
fn motion_off_and_normal_never_report() {
    // 1-based cell sent is (col+1, row+1); cell arg is 0-based (2,3).
    assert!(encode_motion_report(proto(MouseTracking::Off, MouseEncoding::Sgr), true, None, (2, 3)).is_none());
    assert!(encode_motion_report(proto(MouseTracking::Normal, MouseEncoding::Sgr), true, None, (2, 3)).is_none());
}

#[test]
fn motion_button_event_needs_a_held_button() {
    let p = proto(MouseTracking::ButtonEvent, MouseEncoding::Sgr);
    // No button held → no drag report.
    assert!(encode_motion_report(p, false, None, (2, 3)).is_none());
    // Left held → left-drag (button 0 + 32 = 32) at 1-based (3,4).
    assert_eq!(
        encode_motion_report(p, true, None, (2, 3)).unwrap(),
        b"\x1b[<32;3;4M".to_vec()
    );
}

#[test]
fn motion_any_event_reports_bare_and_drag() {
    let p = proto(MouseTracking::AnyEvent, MouseEncoding::Sgr);
    // No button → bare motion (None=3, +32 = 35).
    assert_eq!(
        encode_motion_report(p, false, None, (0, 0)).unwrap(),
        b"\x1b[<35;1;1M".to_vec()
    );
    // Held → left drag.
    assert_eq!(
        encode_motion_report(p, true, None, (0, 0)).unwrap(),
        b"\x1b[<32;1;1M".to_vec()
    );
}

#[test]
fn motion_dedups_within_the_same_cell() {
    let p = proto(MouseTracking::AnyEvent, MouseEncoding::Sgr);
    // Same cell as last_cell → no report.
    assert!(encode_motion_report(p, false, Some((5, 6)), (5, 6)).is_none());
    // Different cell → reports.
    assert!(encode_motion_report(p, false, Some((5, 6)), (5, 7)).is_some());
}

#[test]
fn motion_honours_legacy_encoding() {
    let p = proto(MouseTracking::AnyEvent, MouseEncoding::Default);
    // Bare motion legacy = 35 → button byte 32+35 = 67.
    assert_eq!(encode_motion_report(p, false, None, (0, 0)).unwrap()[3], 67);
}
