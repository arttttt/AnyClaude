//! `ui::popup_view::popup_view` for the SETTINGS popup (E.7.3). Each field is a
//! "[x]"/"[ ]" + label row; the focused row is the highlight block; toggling a
//! field flips its checkbox glyph. Headless through the real term_ui
//! measure/place passes (no GPU).

use std::time::Instant;

use anyclaude::config::{SettingId, SettingSection, SettingsFieldSnapshot};
use anyclaude::ui::app_state::AppState;
use anyclaude::ui::popup_view::{popup_view, POPUP_MIN_WIDTH};
use anyclaude::ui::settings::SettingsDialogState;
use glam::Vec2;
use term_gpu::{FontFamily, FontSystem, TextShapeCache};
use term_ui::{build_root, measure, place_centered, NodeId, NodeKind, RetainedTree, SizeConstraint};

const VIEWPORT: Vec2 = Vec2::new(1000.0, 800.0);

fn field(label: &'static str, value: bool) -> SettingsFieldSnapshot {
    SettingsFieldSnapshot {
        id: SettingId::Agents,
        label,
        description: "desc",
        section: SettingSection::Experimental,
        value,
    }
}

fn app_with_settings(fields: Vec<SettingsFieldSnapshot>, focused: usize) -> AppState {
    let mut state = AppState::new("sid".to_string(), Instant::now(), (80, 24));
    state.settings = SettingsDialogState::Visible {
        fields,
        focused,
        dirty: false,
        confirm_discard: false,
    };
    state
}

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

/// root Block → body Stack → [title, gap, list]; return the list's row nodes.
fn list_rows(tree: &RetainedTree, root: NodeId) -> Vec<NodeId> {
    let body = tree.node(root).children[0];
    let list = tree.node(body).children[2];
    tree.node(list).children.clone()
}

/// A row is either a plain Text (unselected) or a highlight Block wrapping the
/// Text (focused). Returns the row's text either way.
fn row_text(tree: &RetainedTree, node: NodeId) -> String {
    let n = tree.node(node);
    match &n.kind {
        NodeKind::Text(style) => style.text.clone(),
        NodeKind::Block(_) => {
            let inner = n.children[0];
            match &tree.node(inner).kind {
                NodeKind::Text(style) => style.text.clone(),
                other => panic!("highlight block should wrap a Text, got {other:?}"),
            }
        }
        other => panic!("unexpected row kind: {other:?}"),
    }
}

#[test]
fn each_field_is_a_checkbox_row_matching_its_value() {
    let state = app_with_settings(vec![field("Agent Teams", true), field("Beta Thing", false)], 0);
    let (tree, root) = laid_out(&state);
    let rows = list_rows(&tree, root);
    assert_eq!(rows.len(), 2, "one row per field");
    assert!(row_text(&tree, rows[0]).starts_with("[x]"), "a true field shows [x]");
    assert!(row_text(&tree, rows[1]).starts_with("[ ]"), "a false field shows [ ]");
    assert!(row_text(&tree, rows[0]).contains("Agent Teams"), "row carries the label");
}

#[test]
fn focused_row_is_the_highlight_block() {
    let state = app_with_settings(vec![field("A", true), field("B", false), field("C", true)], 1);
    let (tree, root) = laid_out(&state);
    let rows = list_rows(&tree, root);
    assert!(matches!(tree.node(rows[0]).kind, NodeKind::Text(_)), "unfocused row 0 is plain text");
    assert!(matches!(tree.node(rows[1]).kind, NodeKind::Block(_)), "focused row 1 is highlighted");
    assert!(matches!(tree.node(rows[2]).kind, NodeKind::Text(_)), "unfocused row 2 is plain text");
}

#[test]
fn toggling_a_field_flips_its_checkbox_glyph() {
    use anyclaude::ui::settings::SettingsIntent;
    let mut state = app_with_settings(vec![field("Agent Teams", false)], 0);
    {
        let (tree, root) = laid_out(&state);
        assert!(row_text(&tree, list_rows(&tree, root)[0]).starts_with("[ ]"), "starts unchecked");
    }
    state.settings.apply(SettingsIntent::Toggle);
    let (tree, root) = laid_out(&state);
    assert!(
        row_text(&tree, list_rows(&tree, root)[0]).starts_with("[x]"),
        "toggle flips the checkbox glyph"
    );
}

#[test]
fn box_is_min_width_floored_and_has_shadow() {
    let state = app_with_settings(vec![field("A", true)], 0);
    let (tree, root) = laid_out(&state);
    assert!(tree.node(root).measured.x >= POPUP_MIN_WIDTH - 0.5, "min-width floored");
    match &tree.node(root).kind {
        NodeKind::Block(style) => assert!(style.shadow.is_some(), "popup box has a drop shadow"),
        other => panic!("root is the popup box Block, got {other:?}"),
    }
}
