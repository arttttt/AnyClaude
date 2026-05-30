//! Bootstrap smoke tests for `term_layout`. Split / close / resize /
//! hit_test / drag tests land alongside their respective commits.

use term_layout::{PanelId, PanelTree, Split};

#[test]
fn set_focus_moves_focus_to_existing_panel() {
    let mut tree = PanelTree::new(100.0, 100.0);
    let root = tree.panels()[0].0;
    let other = tree.split(root, Split::Horizontal, 0.5).unwrap();
    assert_eq!(tree.focus(), other);
    assert!(tree.set_focus(root));
    assert_eq!(tree.focus(), root);
}

#[test]
fn set_focus_rejects_unknown_id() {
    let mut tree = PanelTree::new(100.0, 100.0);
    let original = tree.focus();
    assert!(!tree.set_focus(PanelId(999)));
    assert_eq!(tree.focus(), original);
}

#[test]
fn new_tree_has_one_full_size_panel() {
    let tree = PanelTree::new(960.0, 600.0);
    let panels = tree.panels();
    assert_eq!(panels.len(), 1, "fresh tree should have a single panel");
    let (id, rect) = panels[0];
    assert_eq!(id, PanelId(0));
    assert_eq!((rect.x, rect.y), (0.0, 0.0));
    assert_eq!((rect.w, rect.h), (960.0, 600.0));
    assert_eq!(tree.focus(), id);
    assert!(!tree.is_empty());
}
