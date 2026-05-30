//! Tests for `PanelTree::split`.

use term_layout::{PanelId, PanelTree, Rect, Split, MIN_RATIO};

fn find(panels: &[(PanelId, Rect)], id: PanelId) -> Rect {
    panels
        .iter()
        .find(|(pid, _)| *pid == id)
        .map(|(_, r)| *r)
        .unwrap_or_else(|| panic!("panel {id:?} not found in {panels:?}"))
}

#[test]
fn horizontal_split_creates_top_and_bottom() {
    let mut tree = PanelTree::new(100.0, 100.0);
    let root = tree.panels()[0].0;
    let new_id = tree.split(root, Split::Horizontal, 0.5).expect("split ok");
    let panels = tree.panels();
    assert_eq!(panels.len(), 2);

    let top = find(&panels, root);
    let bottom = find(&panels, new_id);
    assert_eq!((top.x, top.y, top.w, top.h), (0.0, 0.0, 100.0, 50.0));
    assert_eq!((bottom.x, bottom.y, bottom.w, bottom.h), (0.0, 50.0, 100.0, 50.0));
    // The newly created split takes focus, matching Warp / tmux.
    assert_eq!(tree.focus(), new_id);
}

#[test]
fn vertical_split_creates_left_and_right() {
    let mut tree = PanelTree::new(100.0, 100.0);
    let root = tree.panels()[0].0;
    let new_id = tree.split(root, Split::Vertical, 0.3).expect("split ok");
    let panels = tree.panels();

    let left = find(&panels, root);
    let right = find(&panels, new_id);
    assert!((left.w - 30.0).abs() < 1e-4);
    assert!((right.x - 30.0).abs() < 1e-4);
    assert!((right.w - 70.0).abs() < 1e-4);
    assert_eq!((left.h, right.h), (100.0, 100.0));
}

#[test]
fn nested_split_targets_inner_leaf() {
    let mut tree = PanelTree::new(100.0, 100.0);
    let root = tree.panels()[0].0;
    let bottom = tree
        .split(root, Split::Horizontal, 0.5)
        .expect("first split");
    // Now split the bottom horizontally too — should target the inner
    // leaf, not the original.
    let right_of_bottom = tree
        .split(bottom, Split::Vertical, 0.5)
        .expect("second split");

    let panels = tree.panels();
    assert_eq!(panels.len(), 3);

    let top = find(&panels, root);
    let bottom_left = find(&panels, bottom);
    let bottom_right = find(&panels, right_of_bottom);

    assert_eq!((top.x, top.y, top.w, top.h), (0.0, 0.0, 100.0, 50.0));
    assert_eq!((bottom_left.y, bottom_left.h), (50.0, 50.0));
    assert!((bottom_left.w - 50.0).abs() < 1e-4);
    assert!((bottom_right.x - 50.0).abs() < 1e-4);
    assert!((bottom_right.w - 50.0).abs() < 1e-4);
}

#[test]
fn split_clamps_zero_ratio_to_min() {
    let mut tree = PanelTree::new(100.0, 100.0);
    let root = tree.panels()[0].0;
    let new_id = tree.split(root, Split::Vertical, 0.0).expect("split ok");
    let panels = tree.panels();
    let left = find(&panels, root);
    let right = find(&panels, new_id);
    // Clamped to MIN_RATIO; left gets MIN_RATIO of width, right the rest.
    assert!((left.w - 100.0 * MIN_RATIO).abs() < 1e-4);
    assert!((right.w - 100.0 * (1.0 - MIN_RATIO)).abs() < 1e-4);
}

#[test]
fn split_unknown_target_returns_none() {
    let mut tree = PanelTree::new(100.0, 100.0);
    assert!(tree.split(PanelId(999), Split::Horizontal, 0.5).is_none());
    assert_eq!(tree.panels().len(), 1, "tree should be unchanged");
}
