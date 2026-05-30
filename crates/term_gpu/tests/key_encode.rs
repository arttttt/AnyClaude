//! Keyboard encoder byte tests. The encoder produces exact xterm/VT key
//! sequences; these pin them. `encode_key(key, key_unmod, modifiers, app_cursor)`.

use term_gpu::encode_key;
use winit::keyboard::{Key, ModifiersState, NamedKey};

fn ch(s: &str) -> Key {
    Key::Character(s.into())
}
fn named(n: NamedKey) -> Key {
    Key::Named(n)
}

/// No modifiers, normal cursor mode — the common case.
fn enc(key: &Key, m: ModifiersState) -> Option<Vec<u8>> {
    encode_key(key, key, m, false)
}

#[test]
fn tab_and_shift_tab() {
    assert_eq!(enc(&named(NamedKey::Tab), ModifiersState::empty()), Some(b"\t".to_vec()));
    // Shift+Tab → back-tab CSI Z (mode cycling in ink TUIs).
    assert_eq!(enc(&named(NamedKey::Tab), ModifiersState::SHIFT), Some(b"\x1b[Z".to_vec()));
}

#[test]
fn ctrl_letter_collapses_to_c0() {
    assert_eq!(enc(&ch("c"), ModifiersState::CONTROL), Some(vec![0x03]));
    assert_eq!(enc(&ch("a"), ModifiersState::CONTROL), Some(vec![0x01]));
    // Ctrl+Alt+c → ESC + 0x03.
    assert_eq!(
        enc(&ch("c"), ModifiersState::CONTROL | ModifiersState::ALT),
        Some(vec![0x1b, 0x03])
    );
}

#[test]
fn ctrl_non_letter_combos() {
    assert_eq!(enc(&ch(" "), ModifiersState::CONTROL), Some(vec![0x00]));
    assert_eq!(enc(&ch("["), ModifiersState::CONTROL), Some(vec![0x1b]));
}

#[test]
fn alt_uses_the_uncomposed_base_char() {
    // macOS: the logical key is the composed "å"; the base is "a" → ESC a.
    assert_eq!(
        encode_key(&ch("å"), &ch("a"), ModifiersState::ALT, false),
        Some(vec![0x1b, b'a'])
    );
}

#[test]
fn plain_arrows_normal_vs_application_cursor() {
    assert_eq!(enc(&named(NamedKey::ArrowUp), ModifiersState::empty()), Some(b"\x1b[A".to_vec()));
    // DECCKM (application-cursor-keys) → SS3 form.
    assert_eq!(
        encode_key(&named(NamedKey::ArrowUp), &named(NamedKey::ArrowUp), ModifiersState::empty(), true),
        Some(b"\x1bOA".to_vec())
    );
}

#[test]
fn modified_arrows_use_csi_param_even_under_decckm() {
    // Ctrl+Left = CSI 1 ; 5 D (ctrl bit 4 → param 5); app_cursor is ignored.
    assert_eq!(
        encode_key(&named(NamedKey::ArrowLeft), &named(NamedKey::ArrowLeft), ModifiersState::CONTROL, true),
        Some(b"\x1b[1;5D".to_vec())
    );
    // Shift+Right = CSI 1 ; 2 C.
    assert_eq!(
        enc(&named(NamedKey::ArrowRight), ModifiersState::SHIFT),
        Some(b"\x1b[1;2C".to_vec())
    );
    // Alt+Down = CSI 1 ; 3 B.
    assert_eq!(enc(&named(NamedKey::ArrowDown), ModifiersState::ALT), Some(b"\x1b[1;3B".to_vec()));
}

#[test]
fn home_end_follow_the_cursor_rules() {
    assert_eq!(enc(&named(NamedKey::Home), ModifiersState::empty()), Some(b"\x1b[H".to_vec()));
    assert_eq!(
        encode_key(&named(NamedKey::End), &named(NamedKey::End), ModifiersState::empty(), true),
        Some(b"\x1bOF".to_vec())
    );
    assert_eq!(enc(&named(NamedKey::Home), ModifiersState::CONTROL), Some(b"\x1b[1;5H".to_vec()));
}

#[test]
fn function_keys() {
    assert_eq!(enc(&named(NamedKey::F1), ModifiersState::empty()), Some(b"\x1bOP".to_vec()));
    assert_eq!(enc(&named(NamedKey::F4), ModifiersState::empty()), Some(b"\x1bOS".to_vec()));
    assert_eq!(enc(&named(NamedKey::F5), ModifiersState::empty()), Some(b"\x1b[15~".to_vec()));
    assert_eq!(enc(&named(NamedKey::F12), ModifiersState::empty()), Some(b"\x1b[24~".to_vec()));
    // Modified F5 = CSI 15 ; 2 ~ (shift).
    assert_eq!(enc(&named(NamedKey::F5), ModifiersState::SHIFT), Some(b"\x1b[15;2~".to_vec()));
    // Modified F1 = CSI 1 ; 5 P (ctrl).
    assert_eq!(enc(&named(NamedKey::F1), ModifiersState::CONTROL), Some(b"\x1b[1;5P".to_vec()));
}

#[test]
fn named_fixed_sequences() {
    assert_eq!(enc(&named(NamedKey::Enter), ModifiersState::empty()), Some(b"\r".to_vec()));
    assert_eq!(enc(&named(NamedKey::Escape), ModifiersState::empty()), Some(b"\x1b".to_vec()));
    assert_eq!(enc(&named(NamedKey::Backspace), ModifiersState::empty()), Some(b"\x7f".to_vec()));
    assert_eq!(enc(&named(NamedKey::Delete), ModifiersState::empty()), Some(b"\x1b[3~".to_vec()));
    assert_eq!(enc(&named(NamedKey::Insert), ModifiersState::empty()), Some(b"\x1b[2~".to_vec()));
    assert_eq!(enc(&named(NamedKey::PageUp), ModifiersState::empty()), Some(b"\x1b[5~".to_vec()));
    assert_eq!(enc(&named(NamedKey::PageDown), ModifiersState::empty()), Some(b"\x1b[6~".to_vec()));
}

#[test]
fn printable_and_shifted() {
    assert_eq!(enc(&ch("a"), ModifiersState::empty()), Some(b"a".to_vec()));
    // Shift is already folded into the logical char.
    assert_eq!(enc(&ch("A"), ModifiersState::SHIFT), Some(b"A".to_vec()));
}
