//! Scrollback eviction counter — the anchor input for keeping a scrolled-up
//! viewport stable while output streams (the coordinator reads `lines_evicted`
//! to compensate the from-bottom scroll offset).

use term_core::{TerminalEmulator, VtEmulator};

#[test]
fn scrollback_caps_and_counts_evictions() {
    // 2 visible rows, scrollback capped at 3 → buffer holds at most 5 rows.
    let mut em = VtEmulator::new(10, 2, 3);
    assert_eq!(em.lines_evicted(), 0);

    // Print well past capacity so the top erodes.
    for i in 0..20 {
        em.process(format!("L{i}\r\n").as_bytes());
    }

    // The buffer is capped at visible (2) + scrollback (3).
    assert_eq!(em.snapshot().rows.len(), 5);
    // Lines beyond capacity were evicted off the top and counted.
    assert!(em.lines_evicted() >= 1, "evicted = {}", em.lines_evicted());
}

#[test]
fn no_eviction_under_capacity() {
    // Stays within the buffer → nothing evicted.
    let mut em = VtEmulator::new(10, 2, 50);
    em.process(b"a\r\nb\r\nc\r\n");
    assert_eq!(em.lines_evicted(), 0);
}

#[test]
fn ed3_clear_scrollback_advances_evicted_anchor() {
    // ED 3 (CSI 3 J) drains scrollback. Those lines leave the top of the
    // buffer, so lines_evicted must advance by the drained count — else a
    // scrolled-up viewport loses its anchor.
    let mut em = VtEmulator::new(10, 2, 50);
    for i in 0..6 {
        em.process(format!("L{i}\r\n").as_bytes());
    }
    // Six newlines from a 2-row viewport push ~5 rows into scrollback.
    let scrollback_before = em.snapshot().rows.len() - 2;
    assert!(scrollback_before > 0, "expected non-empty scrollback");
    let evicted_before = em.lines_evicted();

    em.process(b"\x1b[3J"); // ED 3 — clear scrollback

    assert_eq!(
        em.lines_evicted(),
        evicted_before + scrollback_before as u64,
        "ED 3 should advance lines_evicted by the drained scrollback count"
    );
    // Scrollback is gone; only the visible region remains.
    assert_eq!(em.snapshot().rows.len(), 2);
}
