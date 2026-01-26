use crossterm::event::{self, Event, KeyEvent};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

pub enum AppEvent {
    Input(KeyEvent),
    Tick,
    Resize(u16, u16),
    PtyOutput,
}

pub struct EventHandler {
    rx: Receiver<AppEvent>,
    tx: mpsc::Sender<AppEvent>,
}

impl EventHandler {
    pub fn new(tick_rate: Duration) -> Self {
        let (tx, rx) = mpsc::channel();
        let event_tx = tx.clone();

        thread::spawn(move || {
            let mut last_tick = Instant::now();
            loop {
                let timeout = tick_rate.saturating_sub(last_tick.elapsed());
                if event::poll(timeout).unwrap_or(false) {
                    match event::read() {
                        Ok(Event::Key(key)) => {
                            let _ = event_tx.send(AppEvent::Input(key));
                        }
                        Ok(Event::Resize(cols, rows)) => {
                            let _ = event_tx.send(AppEvent::Resize(cols, rows));
                        }
                        Ok(_) => {}
                        Err(_) => break,
                    }
                }

                if last_tick.elapsed() >= tick_rate {
                    let _ = event_tx.send(AppEvent::Tick);
                    last_tick = Instant::now();
                }
            }
        });

        Self { rx, tx }
    }

    pub fn next(&self, timeout: Duration) -> Result<AppEvent, mpsc::RecvTimeoutError> {
        self.rx.recv_timeout(timeout)
    }

    pub fn sender(&self) -> mpsc::Sender<AppEvent> {
        self.tx.clone()
    }
}
