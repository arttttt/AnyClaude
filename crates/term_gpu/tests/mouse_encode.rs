//! Mouse-event encoders (§6 mouse reporting). The bytes are spec-defined
//! (xterm), so these pin the exact output — the one reliable check, since the
//! feature has no consumer to verify against live in a keyboard-driven app.

use term_gpu::{encode_mouse_sgr, encode_mouse_x10};

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
