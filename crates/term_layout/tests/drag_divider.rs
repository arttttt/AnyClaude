//! Tests for `PanelTree::dividers` and `PanelTree::drag_divider`.

use term_layout::{BranchId, PanelTree, Split, MAX_RATIO, MIN_RATIO};

#[test]
fn empty_single_panel_tree_has_no_dividers() {
    let tree = PanelTree::new(100.0, 100.0);
    assert!(tree.dividers().is_empty());
}

#[test]
fn one_divider_per_branch() {
    let mut tree = PanelTree::new(100.0, 100.0);
    let root = tree.panels()[0].0;
    tree.split(root, Split::Horizontal, 0.5).unwrap();
    let dividers = tree.dividers();
    assert_eq!(dividers.len(), 1);

    let d = dividers[0];
    assert_eq!(d.split, Split::Horizontal);
    // Horizontal split → divider is a wide thin horizontal strip
    // sitting at y = 50 px.
    assert!((d.rect.y - 50.0).abs() < 1e-3);
    assert_eq!(d.rect.x, 0.0);
    assert!((d.rect.w - 100.0).abs() < 1e-3);
    assert!((d.rect.h - 1.0).abs() < 1e-3);
}

#[test]
fn vertical_divider_geometry() {
    let mut tree = PanelTree::new(200.0, 80.0);
    let root = tree.panels()[0].0;
    tree.split(root, Split::Vertical, 0.25).unwrap();
    let d = tree.dividers()[0];
    assert_eq!(d.split, Split::Vertical);
    assert!((d.rect.x - 50.0).abs() < 1e-3); // 200 * 0.25
    assert_eq!(d.rect.y, 0.0);
    assert!((d.rect.w - 1.0).abs() < 1e-3);
    assert!((d.rect.h - 80.0).abs() < 1e-3);
}

#[test]
fn drag_divider_changes_child_sizes() {
    let mut tree = PanelTree::new(100.0, 100.0);
    let root = tree.panels()[0].0;
    let bottom = tree.split(root, Split::Horizontal, 0.5).unwrap();
    let divider_id = tree.dividers()[0].id;

    assert!(tree.drag_divider(divider_id, 0.2));
    let panels = tree.panels();
    let top = panels.iter().find(|(id, _)| *id == root).unwrap().1;
    let bot = panels.iter().find(|(id, _)| *id == bottom).unwrap().1;
    assert!((top.h - 20.0).abs() < 1e-3);
    assert!((bot.h - 80.0).abs() < 1e-3);
    assert!((bot.y - 20.0).abs() < 1e-3);
}

#[test]
fn drag_divider_clamps_to_min_max() {
    let mut tree = PanelTree::new(100.0, 100.0);
    let root = tree.panels()[0].0;
    tree.split(root, Split::Vertical, 0.5).unwrap();
    let divider_id = tree.dividers()[0].id;

    // Try to drag past the lower bound.
    tree.drag_divider(divider_id, -0.5);
    let d = tree.dividers()[0];
    assert!((d.rect.x - 100.0 * MIN_RATIO).abs() < 1e-3);

    // Past the upper bound.
    tree.drag_divider(divider_id, 1.5);
    let d = tree.dividers()[0];
    assert!((d.rect.x - 100.0 * MAX_RATIO).abs() < 1e-3);
}

#[test]
fn drag_unknown_branch_returns_false() {
    let mut tree = PanelTree::new(100.0, 100.0);
    let root = tree.panels()[0].0;
    tree.split(root, Split::Horizontal, 0.5).unwrap();
    assert!(!tree.drag_divider(BranchId(999), 0.3));
}

#[test]
fn nested_dividers_have_distinct_ids() {
    let mut tree = PanelTree::new(100.0, 100.0);
    let root = tree.panels()[0].0;
    let bottom = tree.split(root, Split::Horizontal, 0.5).unwrap();
    tree.split(bottom, Split::Vertical, 0.5).unwrap();

    let dividers = tree.dividers();
    assert_eq!(dividers.len(), 2);
    assert_ne!(dividers[0].id, dividers[1].id);
}
