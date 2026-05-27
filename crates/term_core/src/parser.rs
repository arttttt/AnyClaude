//! Paul Williams VT/ANSI parser state machine.
//!
//! Hand-rolled, std-only. The reference state diagram is at
//! <https://vt100.net/emu/dec_ansi_parser>. Sequences supported are the
//! P0 + P1 (and most P2) entries from spec §4.2; DCS/SOS/PM/APC are
//! eaten without dispatch (their state-machine paths exist only so
//! that they don't corrupt subsequent input).
//!
//! The parser is purely zero-allocation on the hot path: byte → state
//! transition → optional `Action::Print(char)` emission. CSI/OSC
//! dispatch points allocate at most for OSC string payloads.

use crate::{CellFlags, TermColor};

const MAX_PARAMS: usize = 16;
const MAX_INTERMEDIATES: usize = 2;
const MAX_OSC_BYTES: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    EscapeIntermediate,
    CsiEntry,
    CsiParam,
    CsiIntermediate,
    CsiIgnore,
    OscString,
    /// DCS family — eaten until ST. We don't implement DECRQSS/etc.
    DcsPassthrough,
    /// SOS / PM / APC strings — eaten until ST.
    SosPmApc,
    /// UTF-8 multi-byte continuation states.
    Utf8_2(u8),
    Utf8_3(u8, u8),
    Utf8_4(u8, u8, u8),
}

/// One unit of work the parser hands to the caller. Almost all variants
/// carry an integer or two; OSC variants own a `String`.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    // C0 controls + printable
    Print(char),
    Bell,
    Backspace,
    Tab,
    LineFeed,
    CarriageReturn,
    /// VT (0x0B) and FF (0x0C). We treat both as LF.
    LineFeedAlt,
    /// 0x1A SUB — abort current escape sequence.
    AbortSequence,

    // Cursor movement
    CursorUp(u16),
    CursorDown(u16),
    CursorForward(u16),
    CursorBack(u16),
    CursorNextLine(u16),
    CursorPrevLine(u16),
    CursorColumn(u16),
    CursorVerticalAbs(u16),
    CursorPosition { row: u16, col: u16 },
    CursorTab(u16),
    CursorBackTab(u16),

    // Erase / edit
    EraseDisplay(EraseMode),
    EraseLine(EraseMode),
    EraseChars(u16),
    InsertChars(u16),
    DeleteChars(u16),
    InsertLines(u16),
    DeleteLines(u16),
    RepeatLast(u16),

    // Scroll
    ScrollUp(u16),
    ScrollDown(u16),
    SetScrollRegion { top: u16, bottom: u16 },

    // SGR
    SetAttr(SgrAction),

    // DEC modes
    DecModeSet(u16),
    DecModeReset(u16),

    // Device status / attributes
    DeviceStatusReport(u16),
    DeviceAttributes,
    /// DECRQM — request mode state. Param is the mode number.
    RequestMode(u16),

    // Cursor save/restore
    SaveCursor,
    RestoreCursor,
    SaveCursorSco,
    RestoreCursorSco,

    // Simple ESC
    Index,        // ESC D
    NextLine,     // ESC E
    ReverseIndex, // ESC M
    KeypadAppMode(bool),
    FullReset,
    SetCursorStyle(u16), // DECSCUSR

    // Tabs
    SetTabStop,        // HTS
    TabClear(TabClear),

    // OSC
    SetTitle(String),
    SetCwd(String),
    Hyperlink {
        /// `id=foo:bar`-style params; empty means none.
        params: String,
        /// Empty URL means close-hyperlink.
        url: String,
    },
    PromptMarker(PromptKind),

    /// Unknown / unsupported but well-formed sequence — useful for
    /// integration testing to flag what real apps emit that we don't
    /// understand.
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EraseMode {
    ToEnd,
    ToStart,
    All,
    Scrollback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabClear {
    Current,
    All,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PromptKind {
    Start,
    End,
    Cont(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SgrAction {
    Reset,
    SetFlag(u16),
    ClearFlag(u16),
    Foreground(TermColor),
    Background(TermColor),
    DefaultForeground,
    DefaultBackground,
}

pub struct Parser {
    state: State,
    params: [u16; MAX_PARAMS],
    /// Parallel array: `true` when the param at the same index was
    /// pushed via `:` (sub-parameter, ITU T.416 style) rather than `;`.
    /// `dispatch_sgr` uses this to distinguish `CSI 4 : 3 m` (curly
    /// underline — single sub-arg on 4) from `CSI 4 ; 3 m` (set
    /// underline THEN italic — two top-level args).
    param_is_sub: [bool; MAX_PARAMS],
    /// Set on `:`; reads when the NEXT param value is pushed.
    next_is_sub: bool,
    param_count: usize,
    current_param: u16,
    private_marker: u8,
    intermediates: [u8; MAX_INTERMEDIATES],
    intermediate_count: usize,
    osc_buf: Vec<u8>,
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

impl Parser {
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            params: [0; MAX_PARAMS],
            param_is_sub: [false; MAX_PARAMS],
            next_is_sub: false,
            param_count: 0,
            current_param: 0,
            private_marker: 0,
            intermediates: [0; MAX_INTERMEDIATES],
            intermediate_count: 0,
            osc_buf: Vec::with_capacity(MAX_OSC_BYTES),
        }
    }

    /// Feed a slice of bytes through the state machine; `emit` is called
    /// for every completed `Action`.
    pub fn advance<F: FnMut(Action)>(&mut self, input: &[u8], mut emit: F) {
        for &byte in input {
            self.feed(byte, &mut emit);
        }
    }

    fn feed<F: FnMut(Action)>(&mut self, byte: u8, emit: &mut F) {
        // Global state escapes (per Paul Williams diagram).
        match byte {
            0x18 | 0x1A => {
                // CAN / SUB — abort current sequence, go to Ground.
                if !matches!(self.state, State::Ground) {
                    emit(Action::AbortSequence);
                }
                self.state = State::Ground;
                return;
            }
            0x1B => {
                // ESC enters Escape from anywhere except OSC/DCS strings
                // (where ESC is the start of ST).
                if matches!(
                    self.state,
                    State::OscString | State::DcsPassthrough | State::SosPmApc
                ) {
                    // ESC may begin ST (ESC \); handled per-state below.
                } else {
                    self.reset_for_escape();
                    self.state = State::Escape;
                    return;
                }
            }
            _ => {}
        }

        match self.state {
            State::Ground => self.ground(byte, emit),
            State::Escape => self.escape(byte, emit),
            State::EscapeIntermediate => self.escape_intermediate(byte, emit),
            State::CsiEntry => self.csi_entry(byte, emit),
            State::CsiParam => self.csi_param(byte, emit),
            State::CsiIntermediate => self.csi_intermediate(byte, emit),
            State::CsiIgnore => self.csi_ignore(byte),
            State::OscString => self.osc_string(byte, emit),
            State::DcsPassthrough => self.dcs_passthrough(byte),
            State::SosPmApc => self.sos_pm_apc(byte),
            State::Utf8_2(b0) => self.utf8_2(b0, byte, emit),
            State::Utf8_3(b0, b1) => self.utf8_3(b0, b1, byte, emit),
            State::Utf8_4(b0, b1, b2) => self.utf8_4(b0, b1, b2, byte, emit),
        }
    }

    fn reset_for_escape(&mut self) {
        self.param_count = 0;
        self.current_param = 0;
        self.next_is_sub = false;
        self.private_marker = 0;
        self.intermediate_count = 0;
        self.osc_buf.clear();
    }

    // ─── Ground ────────────────────────────────────────────────────────────
    fn ground<F: FnMut(Action)>(&mut self, byte: u8, emit: &mut F) {
        match byte {
            0x07 => emit(Action::Bell),
            0x08 => emit(Action::Backspace),
            0x09 => emit(Action::Tab),
            0x0A => emit(Action::LineFeed),
            0x0B | 0x0C => emit(Action::LineFeedAlt),
            0x0D => emit(Action::CarriageReturn),
            0x0E | 0x0F => { /* SO/SI G1/G0 charset — ignored */ }
            0x00..=0x1F => { /* other C0 ignored */ }
            0x20..=0x7E => emit(Action::Print(byte as char)),
            0x7F => { /* DEL */ }
            0xC0..=0xDF => self.state = State::Utf8_2(byte),
            0xE0..=0xEF => self.state = State::Utf8_3(byte, 0),
            0xF0..=0xF4 => self.state = State::Utf8_4(byte, 0, 0),
            _ => { /* invalid UTF-8 lead */ }
        }
    }

    // ─── UTF-8 continuation ────────────────────────────────────────────────
    fn utf8_2<F: FnMut(Action)>(&mut self, b0: u8, byte: u8, emit: &mut F) {
        if byte & 0xC0 == 0x80 {
            let cp = ((b0 as u32 & 0x1F) << 6) | (byte as u32 & 0x3F);
            if let Some(c) = char::from_u32(cp) {
                emit(Action::Print(c));
            }
        }
        self.state = State::Ground;
    }

    fn utf8_3<F: FnMut(Action)>(&mut self, b0: u8, b1: u8, byte: u8, emit: &mut F) {
        if b1 == 0 {
            self.state = State::Utf8_3(b0, byte);
            return;
        }
        if byte & 0xC0 == 0x80 {
            let cp = ((b0 as u32 & 0x0F) << 12)
                | ((b1 as u32 & 0x3F) << 6)
                | (byte as u32 & 0x3F);
            if let Some(c) = char::from_u32(cp) {
                emit(Action::Print(c));
            }
        }
        self.state = State::Ground;
    }

    fn utf8_4<F: FnMut(Action)>(&mut self, b0: u8, b1: u8, b2: u8, byte: u8, emit: &mut F) {
        if b1 == 0 {
            self.state = State::Utf8_4(b0, byte, 0);
            return;
        }
        if b2 == 0 {
            self.state = State::Utf8_4(b0, b1, byte);
            return;
        }
        if byte & 0xC0 == 0x80 {
            let cp = ((b0 as u32 & 0x07) << 18)
                | ((b1 as u32 & 0x3F) << 12)
                | ((b2 as u32 & 0x3F) << 6)
                | (byte as u32 & 0x3F);
            if let Some(c) = char::from_u32(cp) {
                emit(Action::Print(c));
            }
        }
        self.state = State::Ground;
    }

    // ─── Escape ────────────────────────────────────────────────────────────
    fn escape<F: FnMut(Action)>(&mut self, byte: u8, emit: &mut F) {
        match byte {
            b'[' => {
                self.state = State::CsiEntry;
            }
            b']' => {
                self.osc_buf.clear();
                self.state = State::OscString;
            }
            b'P' => {
                self.state = State::DcsPassthrough;
            }
            b'X' | b'^' | b'_' => {
                self.state = State::SosPmApc;
            }
            b'7' => {
                emit(Action::SaveCursor);
                self.state = State::Ground;
            }
            b'8' => {
                emit(Action::RestoreCursor);
                self.state = State::Ground;
            }
            b'D' => {
                emit(Action::Index);
                self.state = State::Ground;
            }
            b'E' => {
                emit(Action::NextLine);
                self.state = State::Ground;
            }
            b'H' => {
                emit(Action::SetTabStop);
                self.state = State::Ground;
            }
            b'M' => {
                emit(Action::ReverseIndex);
                self.state = State::Ground;
            }
            b'=' => {
                emit(Action::KeypadAppMode(true));
                self.state = State::Ground;
            }
            b'>' => {
                emit(Action::KeypadAppMode(false));
                self.state = State::Ground;
            }
            b'c' => {
                emit(Action::FullReset);
                self.state = State::Ground;
            }
            0x20..=0x2F => {
                self.push_intermediate(byte);
                self.state = State::EscapeIntermediate;
            }
            0x30..=0x7E => {
                // Unknown final byte (no intermediates) — ignore.
                emit(Action::Unsupported);
                self.state = State::Ground;
            }
            _ => self.state = State::Ground,
        }
    }

    fn escape_intermediate<F: FnMut(Action)>(&mut self, byte: u8, emit: &mut F) {
        match byte {
            0x20..=0x2F => self.push_intermediate(byte),
            0x30..=0x7E => {
                // e.g. `ESC ( B` (G0 = ASCII). We ignore charset designation
                // for now (G2/G3 explicitly out per spec §4.3).
                let _ = byte;
                emit(Action::Unsupported);
                self.state = State::Ground;
            }
            _ => self.state = State::Ground,
        }
    }

    // ─── CSI ───────────────────────────────────────────────────────────────
    fn csi_entry<F: FnMut(Action)>(&mut self, byte: u8, emit: &mut F) {
        match byte {
            b'?' | b'>' | b'<' | b'=' => {
                self.private_marker = byte;
                self.state = State::CsiParam;
            }
            b'0'..=b'9' => {
                self.current_param = (byte - b'0') as u16;
                self.state = State::CsiParam;
            }
            b';' => {
                self.push_param();
                self.state = State::CsiParam;
            }
            b':' => {
                self.push_param_sub();
                self.state = State::CsiParam;
            }
            0x20..=0x2F => {
                self.push_intermediate(byte);
                self.state = State::CsiIntermediate;
            }
            0x40..=0x7E => {
                self.dispatch_csi(byte, emit);
                self.state = State::Ground;
            }
            _ => self.state = State::CsiIgnore,
        }
    }

    fn csi_param<F: FnMut(Action)>(&mut self, byte: u8, emit: &mut F) {
        match byte {
            b'0'..=b'9' => {
                self.current_param = self
                    .current_param
                    .saturating_mul(10)
                    .saturating_add((byte - b'0') as u16);
            }
            // `;` is the conventional top-level separator. `:` is
            // ITU T.416's sub-parameter form — `CSI 4:0 m` cancels
            // a curly underline; `CSI 38:2:R:G:B m` is colon-form
            // truecolor. Tracking them separately matters because
            // `CSI 4:3 m` (curly underline; sub-arg `3`) must NOT
            // also set ITALIC the way `CSI 4;3 m` (underline THEN
            // italic) does. Mirrors Warp's vte-style ParamsIter.
            b';' => self.push_param(),
            b':' => self.push_param_sub(),
            0x20..=0x2F => {
                self.push_intermediate(byte);
                self.state = State::CsiIntermediate;
            }
            0x40..=0x7E => {
                self.push_param();
                self.dispatch_csi(byte, emit);
                self.state = State::Ground;
            }
            _ => self.state = State::CsiIgnore,
        }
    }

    fn csi_intermediate<F: FnMut(Action)>(&mut self, byte: u8, emit: &mut F) {
        match byte {
            0x20..=0x2F => self.push_intermediate(byte),
            0x40..=0x7E => {
                self.push_param();
                self.dispatch_csi(byte, emit);
                self.state = State::Ground;
            }
            _ => self.state = State::CsiIgnore,
        }
    }

    fn csi_ignore(&mut self, byte: u8) {
        if (0x40..=0x7E).contains(&byte) {
            self.state = State::Ground;
        }
    }

    fn push_param(&mut self) {
        if self.param_count < MAX_PARAMS {
            self.params[self.param_count] = self.current_param;
            self.param_is_sub[self.param_count] = self.next_is_sub;
            self.param_count += 1;
        }
        self.current_param = 0;
        self.next_is_sub = false;
    }

    /// `:` separator — push the current value and mark the NEXT slot
    /// as a sub-parameter (ITU T.416). The handler for the preceding
    /// top-level parameter is responsible for consuming sub-params;
    /// untouched sub-params are skipped by `dispatch_sgr`.
    fn push_param_sub(&mut self) {
        self.push_param();
        self.next_is_sub = true;
    }

    fn push_intermediate(&mut self, byte: u8) {
        if self.intermediate_count < MAX_INTERMEDIATES {
            self.intermediates[self.intermediate_count] = byte;
            self.intermediate_count += 1;
        }
    }

    fn param(&self, idx: usize, default: u16) -> u16 {
        if idx < self.param_count && self.params[idx] != 0 {
            self.params[idx]
        } else {
            default
        }
    }

    fn dispatch_csi<F: FnMut(Action)>(&self, final_byte: u8, emit: &mut F) {
        // Private (`?`) — DEC modes.
        if self.private_marker == b'?' {
            match final_byte {
                b'h' => {
                    for i in 0..self.param_count {
                        emit(Action::DecModeSet(self.params[i]));
                    }
                }
                b'l' => {
                    for i in 0..self.param_count {
                        emit(Action::DecModeReset(self.params[i]));
                    }
                }
                b'p' => emit(Action::RequestMode(self.param(0, 0))),
                _ => {}
            }
            return;
        }
        // Plain CSI.
        match final_byte {
            b'@' => emit(Action::InsertChars(self.param(0, 1))),
            b'A' => emit(Action::CursorUp(self.param(0, 1))),
            b'B' => emit(Action::CursorDown(self.param(0, 1))),
            b'C' => emit(Action::CursorForward(self.param(0, 1))),
            b'D' => emit(Action::CursorBack(self.param(0, 1))),
            b'E' => emit(Action::CursorNextLine(self.param(0, 1))),
            b'F' => emit(Action::CursorPrevLine(self.param(0, 1))),
            b'G' | b'`' => emit(Action::CursorColumn(self.param(0, 1))),
            b'H' | b'f' => emit(Action::CursorPosition {
                row: self.param(0, 1),
                col: self.param(1, 1),
            }),
            b'I' => emit(Action::CursorTab(self.param(0, 1))),
            b'J' => emit(Action::EraseDisplay(self.erase_mode(0))),
            b'K' => emit(Action::EraseLine(self.erase_mode(0))),
            b'L' => emit(Action::InsertLines(self.param(0, 1))),
            b'M' => emit(Action::DeleteLines(self.param(0, 1))),
            b'P' => emit(Action::DeleteChars(self.param(0, 1))),
            b'S' => emit(Action::ScrollUp(self.param(0, 1))),
            b'T' => emit(Action::ScrollDown(self.param(0, 1))),
            b'X' => emit(Action::EraseChars(self.param(0, 1))),
            b'Z' => emit(Action::CursorBackTab(self.param(0, 1))),
            b'b' => emit(Action::RepeatLast(self.param(0, 1))),
            b'c' => emit(Action::DeviceAttributes),
            b'd' => emit(Action::CursorVerticalAbs(self.param(0, 1))),
            b'g' => emit(Action::TabClear(
                if self.param(0, 0) == 3 { TabClear::All } else { TabClear::Current },
            )),
            b'h' => {
                // Public DEC modes (rare). Treat same as private set for now.
                for i in 0..self.param_count {
                    emit(Action::DecModeSet(self.params[i]));
                }
            }
            b'l' => {
                for i in 0..self.param_count {
                    emit(Action::DecModeReset(self.params[i]));
                }
            }
            b'm' => self.dispatch_sgr(emit),
            b'n' => emit(Action::DeviceStatusReport(self.param(0, 0))),
            b'q' if self.intermediate_count == 1 && self.intermediates[0] == b' ' => {
                emit(Action::SetCursorStyle(self.param(0, 1)));
            }
            b'r' => emit(Action::SetScrollRegion {
                top: self.param(0, 1),
                bottom: self.param(1, u16::MAX),
            }),
            b's' => emit(Action::SaveCursorSco),
            b'u' => emit(Action::RestoreCursorSco),
            _ => emit(Action::Unsupported),
        }
    }

    fn erase_mode(&self, idx: usize) -> EraseMode {
        match self.param(idx, 0) {
            0 => EraseMode::ToEnd,
            1 => EraseMode::ToStart,
            2 => EraseMode::All,
            3 => EraseMode::Scrollback,
            _ => EraseMode::ToEnd,
        }
    }

    fn dispatch_sgr<F: FnMut(Action)>(&self, emit: &mut F) {
        if self.param_count == 0 {
            emit(Action::SetAttr(SgrAction::Reset));
            return;
        }
        let mut i = 0;
        while i < self.param_count {
            // Sub-params (`:` separator) are consumed by the
            // handler for the PRECEDING top-level param. Skip any
            // stragglers at the top level so they don't get
            // dispatched as independent SGRs — that's what made
            // `CSI 4:3 m` falsely also set ITALIC.
            if self.param_is_sub[i] {
                i += 1;
                continue;
            }
            let p = self.params[i];
            match p {
                0 => emit(Action::SetAttr(SgrAction::Reset)),
                1 => emit(Action::SetAttr(SgrAction::SetFlag(CellFlags::BOLD))),
                2 => emit(Action::SetAttr(SgrAction::SetFlag(CellFlags::FAINT))),
                3 => emit(Action::SetAttr(SgrAction::SetFlag(CellFlags::ITALIC))),
                4 => {
                    // Sub-parameter form (ITU T.416): the next slot,
                    // when marked as a sub-param of this `4`, picks
                    // the underline STYLE. 4:0 cancels, 4:2 doubles,
                    // 4:1 / 4:3 / 4:4 / 4:5 are single / curly /
                    // dotted / dashed — we collapse the styled
                    // variants to plain UNDERLINE (matching Warp's
                    // `[4, ..] => Attr::Underline`).
                    if i + 1 < self.param_count && self.param_is_sub[i + 1] {
                        let style = self.params[i + 1];
                        match style {
                            0 => emit(Action::SetAttr(SgrAction::ClearFlag(
                                CellFlags::UNDERLINE | CellFlags::DOUBLE_UNDERLINE,
                            ))),
                            2 => emit(Action::SetAttr(SgrAction::SetFlag(
                                CellFlags::DOUBLE_UNDERLINE,
                            ))),
                            _ => {
                                emit(Action::SetAttr(SgrAction::SetFlag(CellFlags::UNDERLINE)))
                            }
                        }
                        // Skip past the consumed sub-param AND any
                        // remaining sub-params on this `4` (some
                        // emitters chain extra sub-args for color).
                        i += 1;
                        while i + 1 < self.param_count && self.param_is_sub[i + 1] {
                            i += 1;
                        }
                    } else if i + 1 < self.param_count
                        && !self.param_is_sub[i + 1]
                        && self.params[i + 1] == 2
                    {
                        // Legacy semicolon-form `CSI 4;2 m` double
                        // underline — keep recognising it.
                        emit(Action::SetAttr(SgrAction::SetFlag(
                            CellFlags::DOUBLE_UNDERLINE,
                        )));
                        i += 1;
                    } else if i + 1 < self.param_count
                        && !self.param_is_sub[i + 1]
                        && self.params[i + 1] == 0
                    {
                        // Legacy semicolon-form `CSI 4;0 m` cancel.
                        emit(Action::SetAttr(SgrAction::ClearFlag(
                            CellFlags::UNDERLINE | CellFlags::DOUBLE_UNDERLINE,
                        )));
                        i += 1;
                    } else {
                        emit(Action::SetAttr(SgrAction::SetFlag(CellFlags::UNDERLINE)));
                    }
                }
                5 => emit(Action::SetAttr(SgrAction::SetFlag(CellFlags::BLINK_SLOW))),
                6 => emit(Action::SetAttr(SgrAction::SetFlag(CellFlags::BLINK_FAST))),
                7 => emit(Action::SetAttr(SgrAction::SetFlag(CellFlags::INVERSE))),
                8 => emit(Action::SetAttr(SgrAction::SetFlag(CellFlags::HIDDEN))),
                9 => emit(Action::SetAttr(SgrAction::SetFlag(CellFlags::STRIKE))),
                21 => emit(Action::SetAttr(SgrAction::ClearFlag(CellFlags::BOLD))),
                22 => emit(Action::SetAttr(SgrAction::ClearFlag(
                    CellFlags::BOLD | CellFlags::FAINT,
                ))),
                23 => emit(Action::SetAttr(SgrAction::ClearFlag(CellFlags::ITALIC))),
                24 => emit(Action::SetAttr(SgrAction::ClearFlag(
                    CellFlags::UNDERLINE | CellFlags::DOUBLE_UNDERLINE,
                ))),
                25 => emit(Action::SetAttr(SgrAction::ClearFlag(
                    CellFlags::BLINK_SLOW | CellFlags::BLINK_FAST,
                ))),
                27 => emit(Action::SetAttr(SgrAction::ClearFlag(CellFlags::INVERSE))),
                28 => emit(Action::SetAttr(SgrAction::ClearFlag(CellFlags::HIDDEN))),
                29 => emit(Action::SetAttr(SgrAction::ClearFlag(CellFlags::STRIKE))),
                30..=37 => emit(Action::SetAttr(SgrAction::Foreground(
                    TermColor::Indexed(p as u8 - 30),
                ))),
                38 => {
                    if let Some(color) = self.parse_extended_color(&mut i) {
                        emit(Action::SetAttr(SgrAction::Foreground(color)));
                    }
                }
                39 => emit(Action::SetAttr(SgrAction::DefaultForeground)),
                40..=47 => emit(Action::SetAttr(SgrAction::Background(
                    TermColor::Indexed(p as u8 - 40),
                ))),
                48 => {
                    if let Some(color) = self.parse_extended_color(&mut i) {
                        emit(Action::SetAttr(SgrAction::Background(color)));
                    }
                }
                49 => emit(Action::SetAttr(SgrAction::DefaultBackground)),
                90..=97 => emit(Action::SetAttr(SgrAction::Foreground(
                    TermColor::Indexed(p as u8 - 90 + 8),
                ))),
                100..=107 => emit(Action::SetAttr(SgrAction::Background(
                    TermColor::Indexed(p as u8 - 100 + 8),
                ))),
                _ => {} // Unknown SGR — ignore.
            }
            i += 1;
        }
    }

    fn parse_extended_color(&self, i: &mut usize) -> Option<TermColor> {
        if *i + 1 >= self.param_count {
            return None;
        }
        match self.params[*i + 1] {
            5 if *i + 2 < self.param_count => {
                *i += 2;
                Some(TermColor::Indexed(self.params[*i] as u8))
            }
            2 if *i + 4 < self.param_count => {
                *i += 4;
                Some(TermColor::Rgb(
                    self.params[*i - 2] as u8,
                    self.params[*i - 1] as u8,
                    self.params[*i] as u8,
                ))
            }
            _ => None,
        }
    }

    // ─── OSC ───────────────────────────────────────────────────────────────
    fn osc_string<F: FnMut(Action)>(&mut self, byte: u8, emit: &mut F) {
        match byte {
            0x07 => {
                // BEL terminator.
                self.dispatch_osc(emit);
                self.state = State::Ground;
            }
            0x1B => {
                // Possible ST (ESC \). Per the simplified state machine, we
                // commit the OSC here and let the next byte (`\`) hit ground.
                self.dispatch_osc(emit);
                self.state = State::Escape;
            }
            0x9C => {
                self.dispatch_osc(emit);
                self.state = State::Ground;
            }
            _ => {
                if self.osc_buf.len() < MAX_OSC_BYTES {
                    self.osc_buf.push(byte);
                }
            }
        }
    }

    fn dispatch_osc<F: FnMut(Action)>(&self, emit: &mut F) {
        let data = &self.osc_buf;
        let Some(sep) = data.iter().position(|&b| b == b';') else {
            return;
        };
        let cmd = &data[..sep];
        let payload = &data[sep + 1..];
        let parse_int = |b: &[u8]| -> Option<u32> {
            std::str::from_utf8(b).ok()?.parse::<u32>().ok()
        };
        let Some(cmd_n) = parse_int(cmd) else {
            return;
        };
        match cmd_n {
            0 | 2 => {
                if let Ok(s) = std::str::from_utf8(payload) {
                    emit(Action::SetTitle(s.to_string()));
                }
            }
            7 => {
                if let Ok(s) = std::str::from_utf8(payload) {
                    emit(Action::SetCwd(s.to_string()));
                }
            }
            8 => {
                // OSC 8 ; params ; url ST. Empty url = close.
                if let Some(sep2) = payload.iter().position(|&b| b == b';') {
                    let params = std::str::from_utf8(&payload[..sep2]).unwrap_or("");
                    let url = std::str::from_utf8(&payload[sep2 + 1..]).unwrap_or("");
                    emit(Action::Hyperlink {
                        params: params.to_string(),
                        url: url.to_string(),
                    });
                }
            }
            133 => {
                // OSC 133 ; A | B | P ; payload?
                let payload_str = std::str::from_utf8(payload).unwrap_or("");
                let mut parts = payload_str.splitn(2, ';');
                let kind = parts.next().unwrap_or("");
                let extra = parts.next().unwrap_or("");
                let pk = match kind {
                    "A" => Some(PromptKind::Start),
                    "B" => Some(PromptKind::End),
                    "P" => Some(PromptKind::Cont(extra.to_string())),
                    _ => None,
                };
                if let Some(pk) = pk {
                    emit(Action::PromptMarker(pk));
                }
            }
            _ => {}
        }
    }

    // ─── DCS / SOS / PM / APC (eaten) ───────────────────────────────────────
    fn dcs_passthrough(&mut self, byte: u8) {
        if byte == 0x9C {
            self.state = State::Ground;
        }
        // ESC is handled by the global escape check above.
    }

    fn sos_pm_apc(&mut self, byte: u8) {
        if byte == 0x9C {
            self.state = State::Ground;
        }
    }
}
