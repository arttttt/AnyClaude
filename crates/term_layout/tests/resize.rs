//! Tests for `PanelTree::resize` — proportional reflow after a window
//! size change.

use term_layout::{PanelId, PanelTree, Rect, Split};

fn find(panels: &[(PanelId, Rect)], id: PanelId) -> Rect {
    panels
        .iter()
        .find(|(pid, _)| *pid == id)
        .map(|(_, r)| *r)
        .unwrap_or_else(|| panic!("panel {id:?} not found in {panels:?}"))
}

#[test]
fn resize_single_panel_fills_new_bounds() {
    let mut tree = PanelTree::new(100.0, 100.0);
    tree.resize(200.0, 80.0);
    let panels = tree.panels();
    assert_eq!(panels.len(), 1);
    let (_, rect) = panels[0];
    assert_eq!((rect.x, rect.y, rect.w, rect.h), (0.0, 0.0, 200.0, 80.0));
}

#[test]
fn resize_preserves_split_ratios() {
    let mut tree = PanelTree::new(100.0, 100.0);
    let root = tree.panels()[0].0;
    let bottom = tree.split(root, Split::Horizontal, 0.25).expect("split");

    tree.resize(200.0, 200.0);
    let panels = tree.panels();
    let top = find(&panels, root);
    let bot = find(&panels, bottom);

    // ratio 0.25 → top should be 50 px tall, bottom 150.
    assert!((top.h - 50.0).abs() < 1e-3, "top h was {}", top.h);
    assert!((bot.h - 150.0).abs() < 1e-3, "bottom h was {}", bot.h);
    assert!((bot.y - 50.0).abs() < 1e-3);
    // Both should span the full width.
    assert!((top.w - 200.0).abs() < 1e-3);
    assert!((bot.w - 200.0).abs() < 1e-3);
}

#[test]
fn resize_nested_splits_preserves_all_ratios() {
    let mut tree = PanelTree::new(100.0, 100.0);
    let root = tree.panels()[0].0;
    let bottom = tree.split(root, Split::Horizontal, 0.5).expect("first split");
    let bottom_right = tree
        .split(bottom, Split::Vertical, 0.5)
        .expect("second split");

    tree.resize(300.0, 200.0);
    let panels = tree.panels();
    let top = find(&panels, root);
    let bl = find(&panels, bottom);
    let br = find(&panels, bottom_right);

    // top is 50% of new height = 100; full width = 300.
    assert!((top.h - 100.0).abs() < 1e-3);
    assert!((top.w - 300.0).abs() < 1e-3);
    // bottom row is the other 100 px; split vertically 50/50 = 150 each.
    assert!((bl.y - 100.0).abs() < 1e-3);
    assert!((bl.h - 100.0).abs() < 1e-3);
    assert!((bl.w - 150.0).abs() < 1e-3);
    assert!((br.x - 150.0).abs() < 1e-3);
    assert!((br.w - 150.0).abs() < 1e-3);
}

#[test]
fn resize_empty_tree_is_noop() {
    let mut tree = PanelTree::new(100.0, 100.0);
    // Will become empty after close lands; for now simulate empty by
    // not initialising — but we ensure resize never panics on the
    // initial tree either.
    tree.resize(50.0, 50.0);
    let panels = tree.panels();
    let (_, rect) = panels[0];
    assert_eq!((rect.w, rect.h), (50.0, 50.0));
}
