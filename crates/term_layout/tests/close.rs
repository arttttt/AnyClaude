//! Tests for `PanelTree::close`.

use term_layout::{PanelId, PanelTree, Split};

#[test]
fn close_single_panel_empties_the_tree() {
    let mut tree = PanelTree::new(100.0, 100.0);
    let root = tree.panels()[0].0;
    tree.close(root);
    assert!(tree.is_empty());
    assert!(tree.panels().is_empty());
}

#[test]
fn close_one_of_two_panels_promotes_sibling() {
    let mut tree = PanelTree::new(100.0, 100.0);
    let root = tree.panels()[0].0;
    let bottom = tree.split(root, Split::Horizontal, 0.5).expect("split");

    // Close the bottom — root should expand back to fill the window.
    tree.close(bottom);
    let panels = tree.panels();
    assert_eq!(panels.len(), 1);
    let (id, rect) = panels[0];
    assert_eq!(id, root);
    assert_eq!((rect.x, rect.y, rect.w, rect.h), (0.0, 0.0, 100.0, 100.0));
}

#[test]
fn close_focused_panel_moves_focus_to_remaining() {
    let mut tree = PanelTree::new(100.0, 100.0);
    let root = tree.panels()[0].0;
    let bottom = tree.split(root, Split::Horizontal, 0.5).expect("split");
    // Split puts focus on the new panel; closing it should move focus
    // back to the original.
    assert_eq!(tree.focus(), bottom);
    tree.close(bottom);
    assert_eq!(tree.focus(), root);
}

#[test]
fn close_from_nested_tree_promotes_sibling_subtree() {
    let mut tree = PanelTree::new(100.0, 100.0);
    let root = tree.panels()[0].0;
    let bottom = tree.split(root, Split::Horizontal, 0.5).expect("split");
    let bottom_right = tree
        .split(bottom, Split::Vertical, 0.5)
        .expect("nested split");

    // Tree layout:
    //   Branch (H, 0.5)
    //     Leaf root        (top)
    //     Branch (V, 0.5)
    //       Leaf bottom    (bottom-left)
    //       Leaf b_right   (bottom-right)
    //
    // Closing the top should promote the lower Branch to take the
    // whole window, preserving the V split between bottom and b_right.
    tree.close(root);
    let panels = tree.panels();
    assert_eq!(panels.len(), 2);

    let find = |id: PanelId| {
        panels
            .iter()
            .find(|(pid, _)| *pid == id)
            .map(|(_, r)| *r)
            .unwrap()
    };
    let bl = find(bottom);
    let br = find(bottom_right);
    assert!((bl.h - 100.0).abs() < 1e-3);
    assert!((br.h - 100.0).abs() < 1e-3);
    assert!((bl.w - 50.0).abs() < 1e-3);
    assert!((br.x - 50.0).abs() < 1e-3);
    assert!((br.w - 50.0).abs() < 1e-3);
}

#[test]
fn close_unknown_panel_is_noop() {
    let mut tree = PanelTree::new(100.0, 100.0);
    let root = tree.panels()[0].0;
    tree.close(PanelId(999));
    assert_eq!(tree.panels().len(), 1);
    assert_eq!(tree.focus(), root);
}
