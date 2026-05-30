//! The backend-switch popup state machine, post-MVI: `BackendSwitchState::apply`
//! is a plain pure transition (no Store/Actor). These pin the ported semantics —
//! section cycling, wrap-around navigation (Active vs the +1 "Disabled" override
//! rows), and Clear resetting only the override sections.

use anyclaude::ui::backend_switch::{BackendPopupSection, BackendSwitchIntent, BackendSwitchState};

fn visible(
    section: BackendPopupSection,
    backend: usize,
    sub: usize,
    team: usize,
    count: usize,
) -> BackendSwitchState {
    BackendSwitchState::Visible {
        section,
        backend_selection: backend,
        subagent_selection: sub,
        teammate_selection: team,
        backends_count: count,
    }
}

/// (backend, subagent, teammate) selections of a Visible state.
fn sel(s: &BackendSwitchState) -> (usize, usize, usize) {
    match s {
        BackendSwitchState::Visible {
            backend_selection,
            subagent_selection,
            teammate_selection,
            ..
        } => (*backend_selection, *subagent_selection, *teammate_selection),
        BackendSwitchState::Hidden => panic!("expected Visible"),
    }
}

fn section(s: &BackendSwitchState) -> BackendPopupSection {
    match s {
        BackendSwitchState::Visible { section, .. } => *section,
        BackendSwitchState::Hidden => panic!("expected Visible"),
    }
}

#[test]
fn open_then_close() {
    let mut s = BackendSwitchState::default();
    assert!(!s.is_visible());
    s.apply(BackendSwitchIntent::Open {
        backend_selection: 2,
        subagent_selection: 1,
        teammate_selection: 0,
        backends_count: 3,
    });
    assert_eq!(s, visible(BackendPopupSection::ActiveBackend, 2, 1, 0, 3));
    s.apply(BackendSwitchIntent::Close);
    assert_eq!(s, BackendSwitchState::Hidden);
}

#[test]
fn next_section_cycles() {
    let mut s = visible(BackendPopupSection::ActiveBackend, 0, 0, 0, 3);
    s.apply(BackendSwitchIntent::NextSection);
    assert_eq!(section(&s), BackendPopupSection::SubagentBackend);
    s.apply(BackendSwitchIntent::NextSection);
    assert_eq!(section(&s), BackendPopupSection::TeammateBackend);
    s.apply(BackendSwitchIntent::NextSection);
    assert_eq!(section(&s), BackendPopupSection::ActiveBackend);
}

#[test]
fn navigate_wraps_in_active_section() {
    let mut s = visible(BackendPopupSection::ActiveBackend, 0, 0, 0, 3);
    s.apply(BackendSwitchIntent::MoveUp); // 0 -> 2 (wrap to last)
    assert_eq!(sel(&s).0, 2);
    s.apply(BackendSwitchIntent::MoveDown); // 2 -> 0 (wrap)
    assert_eq!(sel(&s).0, 0);
    s.apply(BackendSwitchIntent::MoveDown); // 0 -> 1
    assert_eq!(sel(&s).0, 1);
}

#[test]
fn override_section_navigation_includes_disabled_row() {
    // Override rows = backends_count + 1 (index 0 == Disabled), so 3 backends
    // give 4 rows (indices 0..=3).
    let mut s = visible(BackendPopupSection::SubagentBackend, 0, 0, 0, 3);
    s.apply(BackendSwitchIntent::MoveUp); // 0 -> 3 (wrap to last row)
    assert_eq!(sel(&s).1, 3);
    s.apply(BackendSwitchIntent::MoveDown); // 3 -> 0 (wrap)
    assert_eq!(sel(&s).1, 0);
}

#[test]
fn clear_resets_override_but_not_active() {
    // Override section: Clear resets the section's selection to 0 (Disabled).
    let mut s = visible(BackendPopupSection::TeammateBackend, 1, 2, 2, 3);
    s.apply(BackendSwitchIntent::Clear);
    assert_eq!(sel(&s), (1, 2, 0)); // only teammate reset

    // Active section: Clear is a no-op (the active backend can't be cleared).
    let mut a = visible(BackendPopupSection::ActiveBackend, 2, 1, 1, 3);
    a.apply(BackendSwitchIntent::Clear);
    assert_eq!(a, visible(BackendPopupSection::ActiveBackend, 2, 1, 1, 3));
}
