//! Tests for `PanelTree::hit_test`.

use term_layout::{PanelTree, Split};

#[test]
fn hit_center_of_single_panel() {
    let tree = PanelTree::new(100.0, 100.0);
    let root = tree.panels()[0].0;
    assert_eq!(tree.hit_test(50.0, 50.0), Some(root));
}

#[test]
fn hit_top_left_corner_inclusive() {
    let tree = PanelTree::new(100.0, 100.0);
    let root = tree.panels()[0].0;
    assert_eq!(tree.hit_test(0.0, 0.0), Some(root));
}

#[test]
fn hit_bottom_right_corner_exclusive() {
    let tree = PanelTree::new(100.0, 100.0);
    // `(100.0, 100.0)` is the bottom-right of the bounds; bounds are
    // half-open on the right/bottom, so this should miss.
    assert_eq!(tree.hit_test(100.0, 100.0), None);
}

#[test]
fn miss_outside_window() {
    let tree = PanelTree::new(100.0, 100.0);
    assert_eq!(tree.hit_test(-1.0, 50.0), None);
    assert_eq!(tree.hit_test(50.0, 200.0), None);
}

#[test]
fn hit_correct_subpanel_after_split() {
    let mut tree = PanelTree::new(100.0, 100.0);
    let root = tree.panels()[0].0;
    let bottom = tree.split(root, Split::Horizontal, 0.5).expect("split");
    assert_eq!(tree.hit_test(20.0, 10.0), Some(root), "top half hits root");
    assert_eq!(tree.hit_test(20.0, 70.0), Some(bottom), "bottom half hits bottom");
}

#[test]
fn hit_correct_subpanel_after_nested_split() {
    let mut tree = PanelTree::new(100.0, 100.0);
    let root = tree.panels()[0].0;
    let bottom = tree.split(root, Split::Horizontal, 0.5).expect("split");
    let bottom_right = tree
        .split(bottom, Split::Vertical, 0.5)
        .expect("nested split");
    // Layout:
    //   y=0..50:    root      x=0..100
    //   y=50..100:  bottom    x=0..50
    //               br        x=50..100
    assert_eq!(tree.hit_test(25.0, 25.0), Some(root));
    assert_eq!(tree.hit_test(25.0, 75.0), Some(bottom));
    assert_eq!(tree.hit_test(75.0, 75.0), Some(bottom_right));
}
