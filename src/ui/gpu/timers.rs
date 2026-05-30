//! Background timers for the GPU app: the momentum-decay loop, the gesture-end
//! silence fallback, and the 1 Hz chrome heartbeat. Each is a detached thread
//! running an abortable future that sends a [`UserEvent`] through the event-loop
//! proxy until aborted (cancel) or the receiver is gone (window closed). The
//! coordinator owns one [`Timers`] and drives it from the scroll effects + the
//! `resumed` lifecycle hook.

use std::time::Duration;

use futures::future::{abortable, AbortHandle};
use futures_timer::Delay;
use winit::event_loop::EventLoopProxy;

use super::app::UserEvent;

/// Owns the app's abortable timer handles. A `None` handle means that timer is
/// not running; scheduling replaces (and implicitly drops) any prior handle.
pub(super) struct Timers {
    momentum: Option<AbortHandle>,
    gesture_end: Option<AbortHandle>,
    periodic: Option<AbortHandle>,
}

impl Timers {
    pub(super) fn new() -> Self {
        Self { momentum: None, gesture_end: None, periodic: None }
    }

    /// Abort the in-flight momentum loop.
    pub(super) fn cancel_momentum(&mut self) {
        if let Some(a) = self.momentum.take() {
            a.abort();
        }
    }

    /// Abort the pending gesture-end fallback.
    pub(super) fn cancel_gesture_end(&mut self) {
        if let Some(a) = self.gesture_end.take() {
            a.abort();
        }
    }

    /// Start (or restart) the momentum-tick loop firing `MomentumTick` every
    /// `interval`.
    pub(super) fn schedule_momentum(
        &mut self,
        proxy: &EventLoopProxy<UserEvent>,
        interval: Duration,
    ) {
        self.momentum = Some(schedule_loop(proxy.clone(), interval, UserEvent::MomentumTick));
    }

    /// Arm the silence-timeout fallback that fires `GestureEnded` once after
    /// `delay` (for non-precise wheels that never emit `TouchPhase::Ended`).
    pub(super) fn schedule_gesture_end(
        &mut self,
        proxy: &EventLoopProxy<UserEvent>,
        delay: Duration,
    ) {
        self.gesture_end = Some(schedule_once(proxy.clone(), delay, UserEvent::GestureEnded));
    }

    /// Start the 1 Hz `TickRedraw` heartbeat that keeps the chrome (Uptime /
    /// Reqs / sub / team) fresh while the PTY is idle.
    pub(super) fn start_periodic(&mut self, proxy: &EventLoopProxy<UserEvent>) {
        self.periodic =
            Some(schedule_loop(proxy.clone(), Duration::from_secs(1), UserEvent::TickRedraw));
    }
}

/// Spawn a detached thread that fires `event` once after `delay` (abortable).
fn schedule_once(proxy: EventLoopProxy<UserEvent>, delay: Duration, event: UserEvent) -> AbortHandle {
    let (fut, abort) = abortable(async move {
        Delay::new(delay).await;
        let _ = proxy.send_event(event);
    });
    std::thread::spawn(move || {
        let _ = futures::executor::block_on(fut);
    });
    abort
}

/// Spawn a detached thread that fires `event` every `interval` until aborted or
/// the receiver is gone (abortable). Backs both the momentum loop and the
/// periodic heartbeat — they differ only in interval + event.
fn schedule_loop(proxy: EventLoopProxy<UserEvent>, interval: Duration, event: UserEvent) -> AbortHandle {
    let (fut, abort) = abortable(async move {
        loop {
            Delay::new(interval).await;
            if proxy.send_event(event).is_err() {
                break;
            }
        }
    });
    std::thread::spawn(move || {
        let _ = futures::executor::block_on(fut);
    });
    abort
}
