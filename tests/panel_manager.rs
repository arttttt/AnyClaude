//! Unit coverage for `PanelManager` — the one reusable type instantiated per
//! on-screen panel region. Pure state machine; no GPU / window needed.

use anyclaude::ui::panel_manager::{PanelKind, PanelManager, Placement, Policy, RenderMode, Side};

const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

#[test]
fn policies_differ_only_in_data() {
    let left = Policy::sidebar();
    let right = Policy::overlay();
    // The whole point: left vs right is data, not a type.
    assert_eq!(left.side, Side::Left);
    assert_eq!(left.placement, Placement::Displace);
    assert_eq!(left.render, RenderMode::Switcher);
    assert!(!left.resizable);
    assert!(!left.has_indicator);

    assert_eq!(right.side, Side::Right);
    assert_eq!(right.placement, Placement::Overlay);
    assert_eq!(right.render, RenderMode::Stack);
    assert!(right.resizable);
    assert!(right.has_indicator);
}

#[test]
fn starts_empty_collapsed_at_default_width() {
    let m = PanelManager::new(Policy::overlay());
    assert!(m.is_empty());
    assert_eq!(m.len(), 0);
    assert!(!m.is_visible());
    assert_eq!(m.focus(), None);
    assert_eq!(m.width(), Policy::overlay().default_width);
}

#[test]
fn create_issues_monotonic_ids_and_focuses_first() {
    let mut m = PanelManager::new(Policy::overlay());
    let a = m.create(PanelKind::Teammate, "module-mapper", BLUE);
    let b = m.create(PanelKind::Teammate, "flow-tracer", RED);
    assert_ne!(a, b);
    assert_eq!(m.len(), 2);
    // First created panel takes focus; later ones do not steal it.
    assert_eq!(m.focus(), Some(a));
    assert_eq!(m.get(a).unwrap().title, "module-mapper");
    assert_eq!(m.get(b).unwrap().accent, RED);
}

#[test]
fn ids_are_not_reused_after_removal() {
    let mut m = PanelManager::new(Policy::overlay());
    let a = m.create(PanelKind::Teammate, "a", BLUE);
    m.remove(a);
    let b = m.create(PanelKind::Teammate, "b", BLUE);
    assert_ne!(a, b, "a removed id must not be handed out again");
}

#[test]
fn remove_reassigns_focus_to_first_remaining() {
    let mut m = PanelManager::new(Policy::overlay());
    let a = m.create(PanelKind::Teammate, "a", BLUE);
    let b = m.create(PanelKind::Teammate, "b", BLUE);
    let c = m.create(PanelKind::Teammate, "c", BLUE);
    assert_eq!(m.focus(), Some(a));
    m.remove(a); // focused one removed -> falls back to the first remaining (b)
    assert_eq!(m.focus(), Some(b));
    m.remove(c); // not focused -> focus unchanged
    assert_eq!(m.focus(), Some(b));
    m.remove(b); // last one -> focus clears
    assert_eq!(m.focus(), None);
    assert!(m.is_empty());
}

#[test]
fn set_focus_only_accepts_known_ids() {
    let mut m = PanelManager::new(Policy::overlay());
    let a = m.create(PanelKind::Teammate, "a", BLUE);
    let b = m.create(PanelKind::Teammate, "b", BLUE);
    assert!(m.set_focus(b));
    assert_eq!(m.focus(), Some(b));
    m.remove(b);
    assert!(!m.set_focus(b), "focusing a removed panel must fail");
    assert_eq!(m.focus(), Some(a));
}

#[test]
fn reorder_moves_panel_and_clamps() {
    let mut m = PanelManager::new(Policy::overlay());
    m.create(PanelKind::Teammate, "a", BLUE);
    m.create(PanelKind::Teammate, "b", BLUE);
    m.create(PanelKind::Teammate, "c", BLUE);
    m.reorder(0, 2); // a -> end
    let order: Vec<&str> = m.panels().iter().map(|p| p.title.as_str()).collect();
    assert_eq!(order, vec!["b", "c", "a"]);
    // Out-of-range `to` clamps to the last slot; out-of-range `from` is a no-op.
    m.reorder(0, 99);
    let order: Vec<&str> = m.panels().iter().map(|p| p.title.as_str()).collect();
    assert_eq!(order, vec!["c", "a", "b"]);
    m.reorder(99, 0);
    let order: Vec<&str> = m.panels().iter().map(|p| p.title.as_str()).collect();
    assert_eq!(order, vec!["c", "a", "b"], "from out of range is a no-op");
}

#[test]
fn toggle_and_set_visible_flip_visibility() {
    let mut m = PanelManager::new(Policy::overlay());
    assert!(!m.is_visible());
    m.toggle();
    assert!(m.is_visible());
    m.toggle();
    assert!(!m.is_visible());
    m.set_visible(true);
    assert!(m.is_visible());
}

#[test]
fn set_width_clamps_to_policy_bounds() {
    let mut m = PanelManager::new(Policy::overlay());
    let p = Policy::overlay();
    assert_eq!(m.set_width(p.min_width - 100.0), p.min_width);
    assert_eq!(m.set_width(p.max_width + 100.0), p.max_width);
    let mid = (p.min_width + p.max_width) / 2.0;
    assert_eq!(m.set_width(mid), mid);
    assert_eq!(m.width(), mid);
}

#[test]
fn width_is_preserved_across_collapse() {
    let mut m = PanelManager::new(Policy::overlay());
    m.set_width(555.0);
    m.set_visible(true);
    m.toggle(); // collapse
    assert!(!m.is_visible());
    assert_eq!(m.width(), 555.0, "collapsing must not lose the width");
    m.toggle(); // expand
    assert_eq!(m.width(), 555.0);
}

#[test]
fn edge_drag_clamps_to_collapsed_and_max() {
    let p = Policy::overlay();
    let mut m = PanelManager::new(Policy::overlay());
    m.begin_edge_drag();
    m.edge_drag_to(-100.0);
    assert_eq!(m.drag_width(), Some(p.collapsed_width), "floor is the bare strip, not min_width");
    m.edge_drag_to(p.max_width + 1000.0);
    assert_eq!(m.drag_width(), Some(p.max_width));
}

#[test]
fn drag_can_fully_hide_and_reopen() {
    let p = Policy::overlay();
    let mut m = PanelManager::new(Policy::overlay());
    m.create(PanelKind::Teammate, "a", BLUE);
    m.set_width(500.0);
    m.set_visible(true);

    // Drag inward below min_width and release → collapses, width remembered.
    m.begin_edge_drag();
    assert_eq!(m.drag_width(), Some(500.0), "drag from expanded starts at the current width");
    m.edge_drag_to(p.min_width - 50.0);
    m.end_edge_drag();
    assert!(!m.is_visible(), "releasing below min_width hides the overlay");
    assert_eq!(m.drag_width(), None);
    assert_eq!(m.width(), 500.0, "the remembered expand width survives a drag-collapse");

    // From collapsed, drag outward past min_width and release → expands to it.
    m.begin_edge_drag();
    assert_eq!(
        m.drag_width(),
        Some(p.collapsed_width),
        "drag from collapsed starts at the bare strip"
    );
    m.edge_drag_to(640.0);
    m.end_edge_drag();
    assert!(m.is_visible(), "releasing above min_width expands");
    assert_eq!(m.width(), 640.0);
}

#[test]
fn any_active_tracks_running_children() {
    let mut m = PanelManager::new(Policy::overlay());
    let a = m.create(PanelKind::Teammate, "a", BLUE);
    let b = m.create(PanelKind::Teammate, "b", BLUE);
    assert!(!m.any_active(), "placeholders aren't running");
    m.set_running(a, true);
    assert!(m.any_active());
    m.set_running(a, false);
    assert!(!m.any_active());
    m.set_running(b, true);
    assert!(m.any_active());
    m.remove(b);
    assert!(!m.any_active(), "removing the only running panel clears the indicator");
}
