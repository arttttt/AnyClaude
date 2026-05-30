//! `ui::popup_view::popup_view` for the HISTORY popup (E.7.2). Pins the bits the
//! coordinator relies on, headlessly through the real term_ui measure/place
//! passes (no GPU): the list is windowed to `MAX_VISIBLE_ROWS` (R11
//! virtualization), the box is min-width floored + centred, it carries a drop
//! shadow, the selection lands on the right row, and the empty state shows the
//! italic placeholder instead of a list.

use std::time::{Duration, Instant, UNIX_EPOCH};

use anyclaude::ui::app_state::AppState;
use anyclaude::ui::history::{HistoryDialogState, HistoryEntry, MAX_VISIBLE_ROWS};
use anyclaude::ui::popup_view::{popup_view, POPUP_MIN_WIDTH};
use glam::Vec2;
use term_gpu::{FontFamily, FontSystem, TextShapeCache};
use term_ui::{
    build_root, measure, place_centered, NodeId, NodeKind, RetainedTree, SizeConstraint,
};

const VIEWPORT: Vec2 = Vec2::new(1000.0, 800.0);

fn app_with_history(n: usize, scroll_offset: usize) -> AppState {
    let mut state = AppState::new("sid".to_string(), Instant::now(), (80, 24));
    let entries: Vec<HistoryEntry> = (0..n)
        .map(|i| HistoryEntry {
            timestamp: UNIX_EPOCH + Duration::from_secs(1_000 + i as u64),
            from_backend: if i == 0 { None } else { Some(format!("backend{}", i - 1)) },
            to_backend: format!("backend{i}"),
        })
        .collect();
    state.history = HistoryDialogState::Visible { entries, scroll_offset };
    state
}

/// Build the popup view, measure it under the coordinator's constraint
/// (min-width floor, window max), and centre it.
fn laid_out(state: &AppState) -> (RetainedTree, NodeId) {
    let view = popup_view(state).expect("a popup is visible");
    let mut tree = RetainedTree::new();
    let mut fonts = FontSystem::new();
    let mut shape = TextShapeCache::with_family(FontFamily::SansSerif);
    let root = build_root(&mut tree, &view);
    measure(
        &mut tree,
        root,
        SizeConstraint::new(Vec2::new(POPUP_MIN_WIDTH, 0.0), VIEWPORT),
        &mut fonts,
        &mut shape,
        1.0,
    );
    place_centered(&mut tree, root, VIEWPORT);
    (tree, root)
}

/// root Block → body Stack → [title Text, spacer, list-or-placeholder].
fn body_children(tree: &RetainedTree, root: NodeId) -> Vec<NodeId> {
    assert!(matches!(tree.node(root).kind, NodeKind::Block(_)), "root is the popup box Block");
    let body = tree.node(root).children[0];
    assert!(matches!(tree.node(body).kind, NodeKind::Stack(_)), "box wraps a body stack");
    tree.node(body).children.clone()
}

#[test]
fn list_is_windowed_to_max_visible_rows() {
    // 20 entries, scrolled like a freshly-opened dialog (offset = len - window).
    let state = app_with_history(20, 20 - MAX_VISIBLE_ROWS);
    let (tree, root) = laid_out(&state);
    let kids = body_children(&tree, root);
    assert_eq!(kids.len(), 3, "title, gap, list");
    let list = kids[2];
    assert!(matches!(tree.node(list).kind, NodeKind::Stack(_)), "third child is the list");
    assert_eq!(
        tree.node(list).children.len(),
        MAX_VISIBLE_ROWS,
        "only the visible row window is rendered (virtualization), not all 20"
    );
}

#[test]
fn box_is_min_width_floored_and_centered() {
    let state = app_with_history(20, 6);
    let (tree, root) = laid_out(&state);
    let measured = tree.node(root).measured;
    let bounds = tree.node(root).bounds;
    assert!(
        measured.x >= POPUP_MIN_WIDTH - 0.5,
        "box width floored to POPUP_MIN_WIDTH, got {}",
        measured.x
    );
    let expected = ((VIEWPORT - measured) * 0.5).max(Vec2::ZERO);
    assert!((bounds.origin.x - expected.x).abs() < 0.5, "centered x: {} vs {}", bounds.origin.x, expected.x);
    assert!((bounds.origin.y - expected.y).abs() < 0.5, "centered y: {} vs {}", bounds.origin.y, expected.y);
}

#[test]
fn box_carries_a_drop_shadow() {
    let state = app_with_history(3, 0);
    let (tree, root) = laid_out(&state);
    if let NodeKind::Block(style) = &tree.node(root).kind {
        assert!(style.shadow.is_some(), "popup box must carry a drop shadow");
    } else {
        panic!("root is not a Block");
    }
}

#[test]
fn highlight_lands_on_the_top_visible_row() {
    // The reducer clamps scroll_offset to [0, len - MAX_VISIBLE_ROWS], so the
    // window always starts AT scroll_offset (window.start == scroll_offset) and
    // selected_rel = scroll_offset - window.start is always 0 — the highlight is
    // the top visible row. Check both the max valid offset and offset 0.
    for offset in [6, 0] {
        let state = app_with_history(20, offset);
        let (tree, root) = laid_out(&state);
        let list = body_children(&tree, root)[2];
        let rows = tree.node(list).children.clone();
        assert!(
            matches!(tree.node(rows[0]).kind, NodeKind::Block(_)),
            "offset {offset}: top visible row is the highlight"
        );
        for &r in &rows[1..] {
            assert!(
                matches!(tree.node(r).kind, NodeKind::Text(_)),
                "offset {offset}: other rows are plain text"
            );
        }
    }
}

#[test]
fn empty_history_shows_the_italic_placeholder_not_a_list() {
    let state = app_with_history(0, 0);
    let (tree, root) = laid_out(&state);
    let kids = body_children(&tree, root);
    assert_eq!(kids.len(), 3, "title, gap, placeholder");
    let placeholder = kids[2];
    match &tree.node(placeholder).kind {
        NodeKind::Text(style) => {
            assert!(style.italic, "placeholder is italic");
            assert_eq!(style.text, "(no history yet)");
        }
        other => panic!("expected an italic placeholder Text, got {other:?}"),
    }
}
