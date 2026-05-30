//! Mouse-event encoders (§6 mouse reporting). The bytes are spec-defined
//! (xterm), so these pin the exact output — the one reliable check, since the
//! feature has no consumer to verify against live in a keyboard-driven app.

use term_gpu::{
    encode_mouse_report, encode_mouse_sgr, encode_mouse_x10, MouseButton, MouseEventKind,
};

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
