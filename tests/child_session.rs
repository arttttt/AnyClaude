//! `ChildSessionManager` — the registry + lifecycle reacting to typed events,
//! orchestrating a `PanelManager`. Pure state machine; no GPU / window / PTY.

use anyclaude::ui::child_session::{ChildSessionEvent, ChildSessionManager, ChildSpec, PaneId};
use anyclaude::ui::panel_manager::{PanelManager, Policy};

const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

fn spec(name: &str, accent: [f32; 4]) -> ChildSpec {
    ChildSpec { name: name.to_string(), accent }
}

fn register(mgr: &mut ChildSessionManager, panels: &mut PanelManager, name: &str, accent: [f32; 4]) -> PaneId {
    mgr.apply(ChildSessionEvent::Register(spec(name, accent)), panels).expect("register returns a PaneId")
}

#[test]
fn register_creates_a_panel_and_maps_it_both_ways() {
    let mut panels = PanelManager::new(Policy::overlay());
    let mut mgr = ChildSessionManager::new();
    let pane = register(&mut mgr, &mut panels, "module-mapper", RED);

    assert_eq!(panels.len(), 1, "registration creates a panel");
    assert_eq!(mgr.len(), 1, "and a registry entry");
    let panel = mgr.panel_for(pane).expect("pane → panel");
    assert_eq!(panels.get(panel).unwrap().title, "module-mapper");
    assert_eq!(panels.get(panel).unwrap().accent, RED);
    assert_eq!(mgr.pane_for(panel), Some(pane), "bimap reverses");
    assert_eq!(mgr.get(pane).unwrap().name, "module-mapper");
}

#[test]
fn unregister_removes_the_panel_and_entry_and_reassigns_focus() {
    let mut panels = PanelManager::new(Policy::overlay());
    let mut mgr = ChildSessionManager::new();
    let a = register(&mut mgr, &mut panels, "a", BLUE);
    let _b = register(&mut mgr, &mut panels, "b", BLUE);

    let a_panel = mgr.panel_for(a).unwrap();
    assert_eq!(panels.focus(), Some(a_panel), "the first registered panel is focused");

    mgr.apply(ChildSessionEvent::Unregister(a), &mut panels);
    assert_eq!(panels.len(), 1, "its panel is removed");
    assert_eq!(mgr.len(), 1, "its registry entry is dropped");
    assert_eq!(mgr.panel_for(a), None, "the pane no longer resolves");
    assert!(panels.focus().is_some(), "focus falls back to the remaining panel");
}

#[test]
fn pane_ids_are_monotonic_and_not_reused() {
    let mut panels = PanelManager::new(Policy::overlay());
    let mut mgr = ChildSessionManager::new();
    let a = register(&mut mgr, &mut panels, "a", BLUE);
    mgr.apply(ChildSessionEvent::Unregister(a), &mut panels);
    let b = register(&mut mgr, &mut panels, "b", BLUE);
    assert_ne!(a, b, "a freed PaneId is never handed out again");
    assert_eq!(mgr.len(), 1);
}

#[test]
fn unregistering_an_unknown_pane_is_a_noop() {
    let mut panels = PanelManager::new(Policy::overlay());
    let mut mgr = ChildSessionManager::new();
    register(&mut mgr, &mut panels, "a", BLUE);
    mgr.apply(ChildSessionEvent::Unregister(PaneId(9999)), &mut panels);
    assert_eq!(mgr.len(), 1, "no entry removed");
    assert_eq!(panels.len(), 1, "no panel removed");
}
