use crate::pty::PtyHandle;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use termwiz::surface::Surface;

pub struct App {
    should_quit: bool,
    tick_rate: Duration,
    last_tick: Instant,
    show_popup: bool,
    status_message: Option<String>,
    size: Option<(u16, u16)>,
    pty: Option<PtyHandle>,
}

impl App {
    pub fn new(tick_rate: Duration) -> Self {
        Self {
            should_quit: false,
            tick_rate,
            last_tick: Instant::now(),
            show_popup: false,
            status_message: None,
            size: None,
            pty: None,
        }
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn request_quit(&mut self) {
        self.should_quit = true;
    }

    pub fn show_popup(&self) -> bool {
        self.show_popup
    }

    pub fn on_tick(&mut self) {
        self.last_tick = Instant::now();
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        let Some(pty) = &self.pty else {
            return;
        };
        let Some(bytes) = key_event_to_bytes(key) else {
            return;
        };
        let _ = pty.send_input(&bytes);
    }

    pub fn on_resize(&mut self, cols: u16, rows: u16) {
        self.size = Some((cols, rows));
        if let Some(pty) = &self.pty {
            let _ = pty.resize(cols, rows);
        }
    }

    pub fn attach_pty(&mut self, pty: PtyHandle) {
        self.pty = Some(pty);
    }

    pub fn screen(&self) -> Option<Arc<Mutex<Surface>>> {
        self.pty.as_ref().map(|pty| pty.screen())
    }

    #[allow(dead_code)]
    pub fn tick_rate(&self) -> Duration {
        self.tick_rate
    }

    #[allow(dead_code)]
    pub fn last_tick(&self) -> Instant {
        self.last_tick
    }

    #[allow(dead_code)]
    pub fn status_message(&self) -> Option<&str> {
        self.status_message.as_deref()
    }
}

fn key_event_to_bytes(key: KeyEvent) -> Option<Vec<u8>> {
    if key.kind != KeyEventKind::Press {
        return None;
    }

    match key.code {
        KeyCode::Char(c) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                let value = (c as u8).to_ascii_lowercase();
                return Some(vec![value.saturating_sub(b'a') + 1]);
            }
            let mut buffer = [0u8; 4];
            Some(c.encode_utf8(&mut buffer).as_bytes().to_vec())
        }
        KeyCode::Enter => Some(vec![b'\r']),
        KeyCode::Tab => Some(vec![b'\t']),
        KeyCode::Backspace => Some(vec![0x7f]),
        KeyCode::Esc => Some(vec![0x1b]),
        KeyCode::Up => Some(b"\x1b[A".to_vec()),
        KeyCode::Down => Some(b"\x1b[B".to_vec()),
        KeyCode::Right => Some(b"\x1b[C".to_vec()),
        KeyCode::Left => Some(b"\x1b[D".to_vec()),
        KeyCode::Home => Some(b"\x1b[H".to_vec()),
        KeyCode::End => Some(b"\x1b[F".to_vec()),
        KeyCode::PageUp => Some(b"\x1b[5~".to_vec()),
        KeyCode::PageDown => Some(b"\x1b[6~".to_vec()),
        KeyCode::Delete => Some(b"\x1b[3~".to_vec()),
        KeyCode::Insert => Some(b"\x1b[2~".to_vec()),
        _ => None,
    }
}
