//! Parser smoke tests — feed bytes, collect Actions, assert.

use term_core::{Action, CellFlags, EraseMode, Parser, PromptKind, SgrAction, TermColor};

fn collect(input: &[u8]) -> Vec<Action> {
    let mut p = Parser::new();
    let mut out = Vec::new();
    p.advance(input, |a| out.push(a));
    out
}

#[test]
fn prints_ascii() {
    assert_eq!(
        collect(b"abc"),
        vec![
            Action::Print('a'),
            Action::Print('b'),
            Action::Print('c'),
        ]
    );
}

#[test]
fn decodes_utf8_two_byte() {
    // U+00E9 é = 0xC3 0xA9
    assert_eq!(collect(&[0xC3, 0xA9]), vec![Action::Print('é')]);
}

#[test]
fn decodes_utf8_four_byte_emoji() {
    // 🦀 (U+1F980) = F0 9F A6 80
    assert_eq!(collect(&[0xF0, 0x9F, 0xA6, 0x80]), vec![Action::Print('🦀')]);
}

#[test]
fn c0_controls() {
    assert_eq!(collect(b"\x07"), vec![Action::Bell]);
    assert_eq!(collect(b"\x08"), vec![Action::Backspace]);
    assert_eq!(collect(b"\t"), vec![Action::Tab]);
    assert_eq!(collect(b"\n"), vec![Action::LineFeed]);
    assert_eq!(collect(b"\r"), vec![Action::CarriageReturn]);
}

#[test]
fn csi_cursor_moves() {
    assert_eq!(collect(b"\x1b[5A"), vec![Action::CursorUp(5)]);
    assert_eq!(collect(b"\x1b[B"), vec![Action::CursorDown(1)]);
    assert_eq!(
        collect(b"\x1b[3;7H"),
        vec![Action::CursorPosition { row: 3, col: 7 }]
    );
    assert_eq!(
        collect(b"\x1b[H"),
        vec![Action::CursorPosition { row: 1, col: 1 }]
    );
}

#[test]
fn csi_edit_primitives_p0() {
    // ECH / DCH / ICH / REP / VPA — all P0 per research §3.
    assert_eq!(collect(b"\x1b[3X"), vec![Action::EraseChars(3)]);
    assert_eq!(collect(b"\x1b[2P"), vec![Action::DeleteChars(2)]);
    assert_eq!(collect(b"\x1b[5@"), vec![Action::InsertChars(5)]);
    assert_eq!(collect(b"\x1b[4b"), vec![Action::RepeatLast(4)]);
    assert_eq!(collect(b"\x1b[10d"), vec![Action::CursorVerticalAbs(10)]);
    assert_eq!(collect(b"\x1b[2E"), vec![Action::CursorNextLine(2)]);
    assert_eq!(collect(b"\x1b[2F"), vec![Action::CursorPrevLine(2)]);
}

#[test]
fn csi_erase_modes() {
    assert_eq!(collect(b"\x1b[J"), vec![Action::EraseDisplay(EraseMode::ToEnd)]);
    assert_eq!(collect(b"\x1b[2J"), vec![Action::EraseDisplay(EraseMode::All)]);
    assert_eq!(collect(b"\x1b[K"), vec![Action::EraseLine(EraseMode::ToEnd)]);
}

#[test]
fn csi_device_attributes() {
    assert_eq!(collect(b"\x1b[c"), vec![Action::DeviceAttributes]);
}

#[test]
fn csi_dsr() {
    assert_eq!(collect(b"\x1b[6n"), vec![Action::DeviceStatusReport(6)]);
}

#[test]
fn dec_private_modes() {
    assert_eq!(collect(b"\x1b[?25h"), vec![Action::DecModeSet(25)]);
    assert_eq!(collect(b"\x1b[?1049l"), vec![Action::DecModeReset(1049)]);
    assert_eq!(collect(b"\x1b[?7h"), vec![Action::DecModeSet(7)]);
    assert_eq!(collect(b"\x1b[?1004h"), vec![Action::DecModeSet(1004)]);
    assert_eq!(collect(b"\x1b[?2026h"), vec![Action::DecModeSet(2026)]);
}

#[test]
fn sgr_basic() {
    assert_eq!(
        collect(b"\x1b[1;31m"),
        vec![
            Action::SetAttr(SgrAction::SetFlag(CellFlags::BOLD)),
            Action::SetAttr(SgrAction::Foreground(TermColor::Indexed(1))),
        ]
    );
    assert_eq!(collect(b"\x1b[m"), vec![Action::SetAttr(SgrAction::Reset)]);
    assert_eq!(collect(b"\x1b[0m"), vec![Action::SetAttr(SgrAction::Reset)]);
}

#[test]
fn sgr_truecolor_and_indexed() {
    assert_eq!(
        collect(b"\x1b[38;2;255;128;0m"),
        vec![Action::SetAttr(SgrAction::Foreground(TermColor::Rgb(255, 128, 0)))]
    );
    assert_eq!(
        collect(b"\x1b[48;5;42m"),
        vec![Action::SetAttr(SgrAction::Background(TermColor::Indexed(42)))]
    );
}

#[test]
fn sgr_extended_underline() {
    assert_eq!(
        collect(b"\x1b[4;2m"),
        vec![Action::SetAttr(SgrAction::SetFlag(CellFlags::DOUBLE_UNDERLINE))]
    );
}

#[test]
fn cursor_style_decscusr() {
    // DECSCUSR — `CSI Ps SP q`. Space is the intermediate.
    assert_eq!(collect(b"\x1b[3 q"), vec![Action::SetCursorStyle(3)]);
    assert_eq!(collect(b"\x1b[6 q"), vec![Action::SetCursorStyle(6)]);
}

#[test]
fn osc_title_bel_terminator() {
    assert_eq!(
        collect(b"\x1b]0;hello world\x07"),
        vec![Action::SetTitle("hello world".to_string())]
    );
    assert_eq!(
        collect(b"\x1b]2;another title\x07"),
        vec![Action::SetTitle("another title".to_string())]
    );
}

#[test]
fn osc_cwd() {
    assert_eq!(
        collect(b"\x1b]7;file:///Users/me/proj\x07"),
        vec![Action::SetCwd("file:///Users/me/proj".to_string())]
    );
}

#[test]
fn osc_hyperlink_and_close() {
    assert_eq!(
        collect(b"\x1b]8;;https://example.com\x07"),
        vec![Action::Hyperlink {
            params: String::new(),
            url: "https://example.com".to_string(),
        }]
    );
    assert_eq!(
        collect(b"\x1b]8;;\x07"),
        vec![Action::Hyperlink {
            params: String::new(),
            url: String::new(),
        }]
    );
}

#[test]
fn osc_133_prompt_markers() {
    assert_eq!(
        collect(b"\x1b]133;A\x07"),
        vec![Action::PromptMarker(PromptKind::Start)]
    );
    assert_eq!(
        collect(b"\x1b]133;B\x07"),
        vec![Action::PromptMarker(PromptKind::End)]
    );
    assert_eq!(
        collect(b"\x1b]133;P;k=v\x07"),
        vec![Action::PromptMarker(PromptKind::Cont("k=v".to_string()))]
    );
}

#[test]
fn esc_simple_sequences() {
    assert_eq!(collect(b"\x1b7"), vec![Action::SaveCursor]);
    assert_eq!(collect(b"\x1b8"), vec![Action::RestoreCursor]);
    assert_eq!(collect(b"\x1bM"), vec![Action::ReverseIndex]);
    assert_eq!(collect(b"\x1bD"), vec![Action::Index]);
    assert_eq!(collect(b"\x1bE"), vec![Action::NextLine]);
    assert_eq!(collect(b"\x1b="), vec![Action::KeypadAppMode(true)]);
    assert_eq!(collect(b"\x1b>"), vec![Action::KeypadAppMode(false)]);
}

#[test]
fn dcs_body_is_eaten() {
    // ESC P ... ESC \  — DCS body must not produce printable Actions.
    let actions = collect(b"\x1bPq#1234\x1b\\");
    for a in &actions {
        assert!(
            !matches!(a, Action::Print(_)),
            "DCS body emitted Print: {a:?}"
        );
    }
}
