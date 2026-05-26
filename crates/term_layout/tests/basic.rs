//! Bootstrap smoke tests for `term_layout`. Split / close / resize /
//! hit_test / drag tests land alongside their respective commits.

use term_layout::{PanelId, PanelTree};

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
