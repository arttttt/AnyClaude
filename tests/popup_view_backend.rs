//! `ui::popup_view::backend_view` for the BACKEND SWITCH popup (E.7.4). Three
//! sections (Active / Subagent / Teammate); only the active section shows a
//! single highlight; the active backend + current overrides carry green
//! `[Active]` / `[Selected]` status suffixes. Headless through the real term_ui
//! measure/place passes (no GPU).

use anyclaude::ui::backend_switch::{BackendPopupSection, BackendSwitchState};
use anyclaude::ui::popup_view::{backend_view, POPUP_MIN_WIDTH};
use glam::Vec2;
use term_gpu::{FontFamily, FontSystem, TextShapeCache};
use term_ui::{build_root, measure, place_centered, NodeId, NodeKind, RetainedTree, SizeConstraint};

const VIEWPORT: Vec2 = Vec2::new(1200.0, 900.0);
const GREEN: [f32; 4] = [0.4, 0.85, 0.4, 1.0];
const HIGHLIGHT: [f32; 4] = [0.22, 0.30, 0.42, 1.0];

fn backends() -> Vec<(String, String)> {
    vec![
        ("Backend 0".to_string(), "b0".to_string()),
        ("Backend 1".to_string(), "b1".to_string()),
        ("Backend 2".to_string(), "b2".to_string()),
    ]
}

/// State: Subagent section active, cursor on its row 2 (backend "b1").
fn state() -> BackendSwitchState {
    BackendSwitchState::Visible {
        section: BackendPopupSection::SubagentBackend,
        backend_selection: 0,
        subagent_selection: 2,
        teammate_selection: 0,
        backends_count: 3,
    }
}

fn laid_out() -> (RetainedTree, NodeId) {
    // Active backend "b1"; no subagent override (Disabled active); teammate -> "b2".
    let view = backend_view(&state(), &backends(), "b1", None, Some("b2"));
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

/// Collect (text, colour) for every Text node in the subtree.
fn texts(tree: &RetainedTree, node: NodeId, out: &mut Vec<(String, [f32; 4])>) {
    if let NodeKind::Text(s) = &tree.node(node).kind {
        out.push((s.text.clone(), s.color));
    }
    for c in tree.node(node).children.clone() {
        texts(tree, c, out);
    }
}

fn all_texts(tree: &RetainedTree, root: NodeId) -> Vec<(String, [f32; 4])> {
    let mut v = Vec::new();
    texts(tree, root, &mut v);
    v
}

/// Count Block nodes whose background is the selection-highlight colour.
fn highlight_count(tree: &RetainedTree, node: NodeId) -> usize {
    let mut n = 0;
    if let NodeKind::Block(s) = &tree.node(node).kind {
        if s.background == HIGHLIGHT {
            n += 1;
        }
    }
    for c in tree.node(node).children.clone() {
        n += highlight_count(tree, c);
    }
    n
}

#[test]
fn three_section_headers_with_arrow_on_the_active_section() {
    let (tree, root) = laid_out();
    let t = all_texts(&tree, root);
    let has = |s: &str| t.iter().any(|(text, _)| text == s);
    assert!(has("Select Backend"), "title present");
    assert!(has("  Active Backend"), "inactive Active header has no arrow");
    assert!(has("▸ Subagent Backend"), "active Subagent header has the ▸ arrow");
    assert!(has("  Teammate Backend"), "inactive Teammate header has no arrow");
    assert!(
        t.iter().any(|(text, _)| text.starts_with("Tab: Section")),
        "footer hint present"
    );
}

#[test]
fn exactly_one_highlight_in_the_active_section() {
    let (tree, root) = laid_out();
    assert_eq!(
        highlight_count(&tree, root),
        1,
        "only the active (Subagent) section's selected row is highlighted"
    );
}

#[test]
fn status_suffixes_are_green() {
    let (tree, root) = laid_out();
    let t = all_texts(&tree, root);
    // Active backend "b1" carries [Active]; the subagent Disabled leader (no
    // override set) also carries [Active] — both green.
    let active_green = t.iter().filter(|(text, c)| text == "Active" && *c == GREEN).count();
    assert!(active_green >= 1, "[Active] status is green, found {active_green}");
    // Teammate override -> "b2" carries [Selected], green.
    assert!(
        t.iter().any(|(text, c)| text == "Selected" && *c == GREEN),
        "[Selected] status is green"
    );
}

#[test]
fn override_sections_have_the_disabled_leader() {
    let (tree, root) = laid_out();
    let t = all_texts(&tree, root);
    let disabled = t
        .iter()
        .filter(|(text, _)| text == "Disabled (use active backend)")
        .count();
    assert_eq!(disabled, 2, "both override sections lead with a Disabled row");
}

#[test]
fn box_is_min_width_floored_centered_and_has_shadow() {
    let (tree, root) = laid_out();
    let measured = tree.node(root).measured;
    let bounds = tree.node(root).bounds;
    assert!(measured.x >= POPUP_MIN_WIDTH - 0.5, "min-width floored");
    let expected = ((VIEWPORT - measured) * 0.5).max(Vec2::ZERO);
    assert!((bounds.origin.x - expected.x).abs() < 0.5, "centered x");
    assert!((bounds.origin.y - expected.y).abs() < 0.5, "centered y");
    match &tree.node(root).kind {
        NodeKind::Block(style) => assert!(style.shadow.is_some(), "popup box has a drop shadow"),
        other => panic!("root is the popup box Block, got {other:?}"),
    }
}
