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
