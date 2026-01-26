pub mod vt;

use crossterm::terminal::{disable_raw_mode, enable_raw_mode, size as terminal_size};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::cmp::max;
use std::error::Error;
use std::io::{self, Read, Write};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use termwiz::cell::{AttributeChange, CellAttributes};
use termwiz::color::ColorAttribute;
use termwiz::escape::csi::{
    Cursor, CursorStyle, DecPrivateMode, DecPrivateModeCode, Edit, EraseInDisplay, EraseInLine,
    Mode, Sgr, TerminalMode, TerminalModeCode, CSI,
};
use termwiz::escape::{Action, ControlCode, Esc, EscCode, OperatingSystemCommand};
use termwiz::surface::{Change, CursorShape, CursorVisibility, LineAttribute, Position, Surface};
use vt::VtParser;

use crate::ui::events::AppEvent;

#[cfg(unix)]
use signal_hook::consts::signal::SIGWINCH;
#[cfg(unix)]
use signal_hook::iterator::Signals;

pub struct PtyManager {
    vt_parser: VtParser,
    screen: Arc<Mutex<Surface>>,
}

impl PtyManager {
    pub fn new() -> Self {
        let (cols, rows) = terminal_size().unwrap_or((80, 24));
        let screen = Surface::new(usize::from(cols), usize::from(rows));
        Self {
            vt_parser: VtParser::new(),
            screen: Arc::new(Mutex::new(screen)),
        }
    }

    pub fn parse_output(&mut self, bytes: &[u8]) -> Vec<Action> {
        self.vt_parser.parse(bytes)
    }

    pub fn run_command(
        &mut self,
        command: String,
        args: Vec<String>,
    ) -> Result<(), Box<dyn Error>> {
        let pty_system = native_pty_system();
        let (cols, rows) = terminal_size().unwrap_or((80, 24));
        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        self.resize_screen(cols, rows);

        let mut cmd = CommandBuilder::new(command);
        cmd.args(args);
        cmd.cwd(std::env::current_dir()?);

        let mut child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let raw_mode_guard = RawModeGuard::new()?;

        let master = pair.master;
        let reader = master.try_clone_reader()?;
        let writer = master.take_writer()?;
        let resize_master = Arc::new(Mutex::new(master));
        let resize_watcher =
            ResizeWatcher::start(Arc::clone(&resize_master), Arc::clone(&self.screen))?;

        let reader_handle = thread::spawn(move || {
            let mut reader = reader;
            let mut stdout = io::stdout();
            let _ = io::copy(&mut reader, &mut stdout);
            let _ = stdout.flush();
        });

        let _writer_handle = thread::spawn(move || {
            let mut stdin = io::stdin();
            let mut writer = writer;
            let mut buffer = [0u8; 1024];

            loop {
                let read_bytes = match stdin.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => count,
                    Err(_) => break,
                };

                let mut filtered = Vec::with_capacity(read_bytes);
                for &byte in &buffer[..read_bytes] {
                    if is_wrapper_hotkey(byte) {
                        continue;
                    }
                    filtered.push(byte);
                }

                if filtered.is_empty() {
                    continue;
                }

                if writer.write_all(&filtered).is_err() {
                    break;
                }
                if writer.flush().is_err() {
                    break;
                }
            }
        });

        let status = child.wait()?;
        drop(raw_mode_guard);
        if let Some(watcher) = resize_watcher {
            watcher.stop();
        }
        let _ = reader_handle.join();

        if status.success() {
            return Ok(());
        }

        std::process::exit(status.exit_code() as i32);
    }

    fn resize_screen(&self, cols: u16, rows: u16) {
        if let Ok(mut screen) = self.screen.lock() {
            screen.resize(usize::from(cols), usize::from(rows));
        }
    }
}

#[derive(Clone)]
pub struct PtyHandle {
    screen: Arc<Mutex<Surface>>,
    writer: Arc<Mutex<Option<Box<dyn Write + Send>>>>,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
}

impl PtyHandle {
    pub fn screen(&self) -> Arc<Mutex<Surface>> {
        Arc::clone(&self.screen)
    }

    pub fn send_input(&self, bytes: &[u8]) -> io::Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "input writer lock poisoned"))?;
        let Some(writer) = writer.as_mut() else {
            return Ok(());
        };
        let mut filtered = Vec::with_capacity(bytes.len());
        for &byte in bytes {
            if is_wrapper_hotkey(byte) {
                continue;
            }
            filtered.push(byte);
        }
        if filtered.is_empty() {
            return Ok(());
        }
        writer.write_all(&filtered)?;
        writer.flush()?;
        Ok(())
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), Box<dyn Error>> {
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        if let Ok(master) = self.master.lock() {
            master.resize(size)?;
        }
        if let Ok(mut screen) = self.screen.lock() {
            screen.resize(usize::from(cols), usize::from(rows));
        }
        Ok(())
    }

    fn close_writer(&self) {
        if let Ok(mut writer) = self.writer.lock() {
            *writer = None;
        }
    }
}

pub struct PtySession {
    handle: PtyHandle,
    child: Box<dyn Child + Send + Sync>,
    reader_handle: Option<thread::JoinHandle<()>>,
}

impl PtySession {
    pub fn spawn(
        command: String,
        args: Vec<String>,
        notifier: Sender<AppEvent>,
    ) -> Result<Self, Box<dyn Error>> {
        let pty_system = native_pty_system();
        let (cols, rows) = terminal_size().unwrap_or((80, 24));
        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let screen = Arc::new(Mutex::new(Surface::new(
            usize::from(cols),
            usize::from(rows),
        )));

        let mut cmd = CommandBuilder::new(command);
        cmd.args(args);
        cmd.cwd(std::env::current_dir()?);
        cmd.env("TERM", "xterm-256color");

        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let master = Arc::new(Mutex::new(pair.master));
        let handle = PtyHandle {
            screen: Arc::clone(&screen),
            writer: Arc::new(Mutex::new(Some(writer))),
            master,
        };

        let reader_handle = thread::spawn(move || {
            let mut reader = reader;
            let mut parser = VtParser::new();
            let mut translator = ActionTranslator::new();
            let mut buffer = [0u8; 8192];

            loop {
                let count = match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => count,
                    Err(_) => break,
                };
                let actions = parser.parse(&buffer[..count]);
                if let Ok(mut screen) = screen.lock() {
                    apply_actions(&mut screen, &mut translator, actions);
                }
                let _ = notifier.send(AppEvent::PtyOutput);
            }
        });

        Ok(Self {
            handle,
            child,
            reader_handle: Some(reader_handle),
        })
    }

    pub fn handle(&self) -> PtyHandle {
        self.handle.clone()
    }

    pub fn shutdown(&mut self) -> Result<(), Box<dyn Error>> {
        self.handle.close_writer();
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader_handle) = self.reader_handle.take() {
            let _ = reader_handle.join();
        }
        Ok(())
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

pub fn parse_command() -> (String, Vec<String>) {
    let args: Vec<String> = std::env::args().skip(1).collect();
    parse_command_from(args)
}

pub fn parse_command_from(args: Vec<String>) -> (String, Vec<String>) {
    let command = "claude".to_string();
    (command, args)
}

fn is_wrapper_hotkey(byte: u8) -> bool {
    byte == 0x02 || byte == 0x13 || byte == 0x11
}

struct ActionTranslator {
    saved_cursor: Option<(usize, usize)>,
}

impl ActionTranslator {
    fn new() -> Self {
        Self { saved_cursor: None }
    }
}

fn apply_actions(screen: &mut Surface, translator: &mut ActionTranslator, actions: Vec<Action>) {
    for action in actions {
        let mut changes = Vec::new();
        translate_action(screen, translator, action, &mut changes);
        if !changes.is_empty() {
            screen.add_changes(changes);
        }
    }
}

fn translate_action(
    screen: &Surface,
    translator: &mut ActionTranslator,
    action: Action,
    changes: &mut Vec<Change>,
) {
    match action {
        Action::Print(ch) => changes.push(Change::Text(ch.to_string())),
        Action::PrintString(text) => changes.push(Change::Text(text)),
        Action::Control(code) => translate_control(code, changes),
        Action::CSI(csi) => translate_csi(screen, translator, csi, changes),
        Action::Esc(esc) => translate_esc(screen, translator, esc, changes),
        Action::OperatingSystemCommand(osc) => translate_osc(osc, changes),
        _ => {}
    }
}

fn translate_control(code: ControlCode, changes: &mut Vec<Change>) {
    match code {
        ControlCode::LineFeed | ControlCode::VerticalTab | ControlCode::FormFeed => {
            changes.push(Change::Text("\n".to_string()));
        }
        ControlCode::CarriageReturn => changes.push(Change::Text("\r".to_string())),
        ControlCode::Backspace => changes.push(Change::CursorPosition {
            x: Position::Relative(-1),
            y: Position::Relative(0),
        }),
        ControlCode::HorizontalTab => changes.push(Change::Text("        ".to_string())),
        _ => {}
    }
}

fn translate_csi(
    screen: &Surface,
    translator: &mut ActionTranslator,
    csi: CSI,
    changes: &mut Vec<Change>,
) {
    match csi {
        CSI::Sgr(sgr) => {
            if let Some(change) = translate_sgr(sgr) {
                changes.push(change);
            }
        }
        CSI::Cursor(cursor) => translate_cursor(screen, translator, cursor, changes),
        CSI::Edit(edit) => translate_edit(screen, edit, changes),
        CSI::Mode(mode) => translate_mode(mode, changes),
        _ => {}
    }
}

fn translate_sgr(sgr: Sgr) -> Option<Change> {
    match sgr {
        Sgr::Reset => Some(Change::AllAttributes(CellAttributes::default())),
        Sgr::Intensity(value) => Some(Change::Attribute(AttributeChange::Intensity(value))),
        Sgr::Underline(value) => Some(Change::Attribute(AttributeChange::Underline(value))),
        Sgr::Italic(value) => Some(Change::Attribute(AttributeChange::Italic(value))),
        Sgr::Blink(value) => Some(Change::Attribute(AttributeChange::Blink(value))),
        Sgr::Inverse(value) => Some(Change::Attribute(AttributeChange::Reverse(value))),
        Sgr::Invisible(value) => Some(Change::Attribute(AttributeChange::Invisible(value))),
        Sgr::StrikeThrough(value) => Some(Change::Attribute(AttributeChange::StrikeThrough(value))),
        Sgr::Foreground(color) => Some(Change::Attribute(AttributeChange::Foreground(
            ColorAttribute::from(color),
        ))),
        Sgr::Background(color) => Some(Change::Attribute(AttributeChange::Background(
            ColorAttribute::from(color),
        ))),
        Sgr::UnderlineColor(_) | Sgr::Overline(_) | Sgr::VerticalAlign(_) | Sgr::Font(_) => None,
    }
}

fn translate_cursor(
    screen: &Surface,
    translator: &mut ActionTranslator,
    cursor: Cursor,
    changes: &mut Vec<Change>,
) {
    match cursor {
        Cursor::Up(count) => changes.push(Change::CursorPosition {
            x: Position::Relative(0),
            y: Position::Relative(-(count as isize)),
        }),
        Cursor::Down(count) => changes.push(Change::CursorPosition {
            x: Position::Relative(0),
            y: Position::Relative(count as isize),
        }),
        Cursor::Left(count) => changes.push(Change::CursorPosition {
            x: Position::Relative(-(count as isize)),
            y: Position::Relative(0),
        }),
        Cursor::Right(count) => changes.push(Change::CursorPosition {
            x: Position::Relative(count as isize),
            y: Position::Relative(0),
        }),
        Cursor::Position { line, col } | Cursor::CharacterAndLinePosition { line, col } => {
            changes.push(Change::CursorPosition {
                x: Position::Absolute(col.as_zero_based() as usize),
                y: Position::Absolute(line.as_zero_based() as usize),
            });
        }
        Cursor::CharacterAbsolute(col) | Cursor::CharacterPositionAbsolute(col) => {
            changes.push(Change::CursorPosition {
                x: Position::Absolute(col.as_zero_based() as usize),
                y: Position::Relative(0),
            });
        }
        Cursor::CharacterPositionBackward(count) => changes.push(Change::CursorPosition {
            x: Position::Relative(-(count as isize)),
            y: Position::Relative(0),
        }),
        Cursor::CharacterPositionForward(count) => changes.push(Change::CursorPosition {
            x: Position::Relative(count as isize),
            y: Position::Relative(0),
        }),
        Cursor::LinePositionAbsolute(count) => changes.push(Change::CursorPosition {
            x: Position::Relative(0),
            y: Position::Absolute(max(1, count) as usize - 1),
        }),
        Cursor::LinePositionBackward(count) => changes.push(Change::CursorPosition {
            x: Position::Relative(0),
            y: Position::Relative(-(count as isize)),
        }),
        Cursor::LinePositionForward(count) => changes.push(Change::CursorPosition {
            x: Position::Relative(0),
            y: Position::Relative(count as isize),
        }),
        Cursor::NextLine(count) => changes.push(Change::CursorPosition {
            x: Position::Absolute(0),
            y: Position::Relative(count as isize),
        }),
        Cursor::PrecedingLine(count) => changes.push(Change::CursorPosition {
            x: Position::Absolute(0),
            y: Position::Relative(-(count as isize)),
        }),
        Cursor::SaveCursor => translator.saved_cursor = Some(screen.cursor_position()),
        Cursor::RestoreCursor => {
            if let Some((x, y)) = translator.saved_cursor {
                changes.push(Change::CursorPosition {
                    x: Position::Absolute(x),
                    y: Position::Absolute(y),
                });
            }
        }
        Cursor::CursorStyle(style) => {
            if let Some(shape) = cursor_shape_from_style(style) {
                changes.push(Change::CursorShape(shape));
            }
        }
        _ => {}
    }
}

fn cursor_shape_from_style(style: CursorStyle) -> Option<CursorShape> {
    match style {
        CursorStyle::Default => Some(CursorShape::Default),
        CursorStyle::BlinkingBlock => Some(CursorShape::BlinkingBlock),
        CursorStyle::SteadyBlock => Some(CursorShape::SteadyBlock),
        CursorStyle::BlinkingUnderline => Some(CursorShape::BlinkingUnderline),
        CursorStyle::SteadyUnderline => Some(CursorShape::SteadyUnderline),
        CursorStyle::BlinkingBar => Some(CursorShape::BlinkingBar),
        CursorStyle::SteadyBar => Some(CursorShape::SteadyBar),
    }
}

fn translate_edit(screen: &Surface, edit: Edit, changes: &mut Vec<Change>) {
    let (_width, height) = screen.dimensions();
    match edit {
        Edit::EraseInLine(EraseInLine::EraseToEndOfLine) => {
            changes.push(Change::ClearToEndOfLine(Default::default()));
        }
        Edit::EraseInLine(EraseInLine::EraseToStartOfLine) => {
            erase_to_start_of_line(screen, changes);
        }
        Edit::EraseInLine(EraseInLine::EraseLine) => {
            erase_entire_line(screen, changes);
        }
        Edit::EraseInDisplay(EraseInDisplay::EraseToEndOfDisplay) => {
            changes.push(Change::ClearToEndOfScreen(Default::default()));
        }
        Edit::EraseInDisplay(EraseInDisplay::EraseDisplay) => {
            changes.push(Change::ClearScreen(Default::default()));
        }
        Edit::ScrollUp(count) => changes.push(Change::ScrollRegionUp {
            first_row: 0,
            region_size: height,
            scroll_count: count as usize,
        }),
        Edit::ScrollDown(count) => changes.push(Change::ScrollRegionDown {
            first_row: 0,
            region_size: height,
            scroll_count: count as usize,
        }),
        _ => {}
    }
}

fn erase_to_start_of_line(screen: &Surface, changes: &mut Vec<Change>) {
    let (cursor_x, cursor_y) = screen.cursor_position();
    let count = cursor_x.saturating_add(1);
    if count == 0 {
        return;
    }
    changes.push(Change::CursorPosition {
        x: Position::Absolute(0),
        y: Position::Absolute(cursor_y),
    });
    changes.push(Change::Text(" ".repeat(count)));
    changes.push(Change::CursorPosition {
        x: Position::Absolute(cursor_x),
        y: Position::Absolute(cursor_y),
    });
}

fn erase_entire_line(screen: &Surface, changes: &mut Vec<Change>) {
    let (width, _height) = screen.dimensions();
    let (cursor_x, cursor_y) = screen.cursor_position();
    if width == 0 {
        return;
    }
    changes.push(Change::CursorPosition {
        x: Position::Absolute(0),
        y: Position::Absolute(cursor_y),
    });
    changes.push(Change::Text(" ".repeat(width)));
    changes.push(Change::CursorPosition {
        x: Position::Absolute(cursor_x),
        y: Position::Absolute(cursor_y),
    });
}

fn translate_mode(mode: Mode, changes: &mut Vec<Change>) {
    match mode {
        Mode::SetDecPrivateMode(DecPrivateMode::Code(DecPrivateModeCode::ShowCursor))
        | Mode::SetMode(TerminalMode::Code(TerminalModeCode::ShowCursor)) => {
            changes.push(Change::CursorVisibility(CursorVisibility::Visible));
        }
        Mode::ResetDecPrivateMode(DecPrivateMode::Code(DecPrivateModeCode::ShowCursor))
        | Mode::ResetMode(TerminalMode::Code(TerminalModeCode::ShowCursor)) => {
            changes.push(Change::CursorVisibility(CursorVisibility::Hidden));
        }
        _ => {}
    }
}

fn translate_esc(
    screen: &Surface,
    translator: &mut ActionTranslator,
    esc: Esc,
    changes: &mut Vec<Change>,
) {
    let Esc::Code(code) = esc else {
        return;
    };

    match code {
        EscCode::Index => changes.push(Change::Text("\n".to_string())),
        EscCode::NextLine => changes.push(Change::Text("\r\n".to_string())),
        EscCode::ReverseIndex => changes.push(Change::CursorPosition {
            x: Position::Relative(0),
            y: Position::Relative(-1),
        }),
        EscCode::CursorPositionLowerLeft => {
            let (_width, height) = screen.dimensions();
            if height > 0 {
                changes.push(Change::CursorPosition {
                    x: Position::Absolute(0),
                    y: Position::Absolute(height.saturating_sub(1)),
                });
            }
        }
        EscCode::DecSaveCursorPosition => translator.saved_cursor = Some(screen.cursor_position()),
        EscCode::DecRestoreCursorPosition => {
            if let Some((x, y)) = translator.saved_cursor {
                changes.push(Change::CursorPosition {
                    x: Position::Absolute(x),
                    y: Position::Absolute(y),
                });
            }
        }
        EscCode::DecDoubleHeightTopHalfLine => {
            changes.push(Change::LineAttribute(
                LineAttribute::DoubleHeightTopHalfLine,
            ));
        }
        EscCode::DecDoubleHeightBottomHalfLine => {
            changes.push(Change::LineAttribute(
                LineAttribute::DoubleHeightBottomHalfLine,
            ));
        }
        EscCode::DecSingleWidthLine => {
            changes.push(Change::LineAttribute(LineAttribute::SingleWidthLine));
        }
        EscCode::DecDoubleWidthLine => {
            changes.push(Change::LineAttribute(LineAttribute::DoubleWidthLine));
        }
        EscCode::FullReset => {
            changes.push(Change::AllAttributes(CellAttributes::default()));
            changes.push(Change::ClearScreen(Default::default()));
        }
        _ => {}
    }
}

fn translate_osc(osc: Box<OperatingSystemCommand>, changes: &mut Vec<Change>) {
    match *osc {
        OperatingSystemCommand::SetWindowTitle(title)
        | OperatingSystemCommand::SetIconNameAndWindowTitle(title)
        | OperatingSystemCommand::SetWindowTitleSun(title)
        | OperatingSystemCommand::SetIconName(title)
        | OperatingSystemCommand::SetIconNameSun(title) => {
            changes.push(Change::Title(title));
        }
        _ => {}
    }
}

struct RawModeGuard;

impl RawModeGuard {
    fn new() -> Result<Self, Box<dyn Error>> {
        enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

struct ResizeWatcher {
    #[cfg(unix)]
    handle: signal_hook::iterator::Handle,
    #[cfg(unix)]
    thread: thread::JoinHandle<()>,
}

impl ResizeWatcher {
    fn start(
        master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
        screen: Arc<Mutex<Surface>>,
    ) -> Result<Option<Self>, Box<dyn Error>> {
        #[cfg(unix)]
        {
            let mut signals = Signals::new([SIGWINCH])?;
            let handle = signals.handle();
            let thread = thread::spawn(move || {
                for _ in signals.forever() {
                    let (cols, rows) = match terminal_size() {
                        Ok(size) => size,
                        Err(_) => continue,
                    };
                    let size = PtySize {
                        rows,
                        cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    };
                    if let Ok(master) = master.lock() {
                        let _ = master.resize(size);
                    }
                    if let Ok(mut screen) = screen.lock() {
                        screen.resize(usize::from(cols), usize::from(rows));
                    }
                }
            });
            return Ok(Some(Self { handle, thread }));
        }

        #[cfg(not(unix))]
        {
            let _ = master;
            let _ = screen;
            Ok(None)
        }
    }

    fn stop(self) {
        #[cfg(unix)]
        {
            self.handle.close();
            let _ = self.thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_command_from;

    #[test]
    fn parse_command_defaults_to_claude() {
        let (command, args) = parse_command_from(Vec::new());
        assert_eq!(command, "claude");
        assert!(args.is_empty());
    }

    #[test]
    fn parse_command_with_args() {
        let args = vec!["--debug".to_string(), "--model".to_string()];
        let (command, remaining) = parse_command_from(args);
        assert_eq!(command, "claude");
        assert_eq!(
            remaining,
            vec!["--debug".to_string(), "--model".to_string()]
        );
    }
}
