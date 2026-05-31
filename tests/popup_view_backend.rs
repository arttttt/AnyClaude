//! `ui::popup_view::backend_view` for the BACKEND SWITCH popup (E.7.4). Three
//! sections (Active / Subagent / Teammate); only the active section shows a
//! single highlight; the active backend + current overrides carry green
//! `[Active]` / `[Selected]` status suffixes. Headless through the real term_ui
//! measure/place passes (no GPU).

use anyclaude::ui::backend_switch::{BackendPopupSection, BackendSwitchState};
use anyclaude::ui::popup_view::{backend_view, POPUP_MIN_WIDTH};
use glam::Vec2;
use term_gpu::{FontFamily, FontSystem, TextShapeCache};
use term_ui::{
    build_root, measure, place_centered, Mod, NodeId, NodeKind, RetainedTree, SizeConstraint,
};

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
    if let NodeKind::Modified(m) = &tree.node(node).kind {
        if m.ops.contains(&Mod::Background(HIGHLIGHT)) {
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
fn highlight_is_in_the_active_section_at_the_selected_row() {
    let (tree, root) = laid_out();
    // root Block → body Stack → [title, gap, active, gap, subagent, gap, teammate, gap, footer].
    let body = tree.node(root).children[0];
    let body_kids = tree.node(body).children.clone();
    let (active, subagent, teammate) = (body_kids[2], body_kids[4], body_kids[6]);
    assert_eq!(highlight_count(&tree, active), 0, "inactive Active section: no highlight");
    assert_eq!(highlight_count(&tree, teammate), 0, "inactive Teammate section: no highlight");
    assert_eq!(highlight_count(&tree, subagent), 1, "active Subagent section: exactly one highlight");
    // Subagent section children: [header, separator, row0(Disabled), row1, row2, row3].
    // subagent_selection = 2 → the selected row is row2, at child index 2 + 2 = 4.
    let sub_kids = tree.node(subagent).children.clone();
    assert!(
        matches!(tree.node(sub_kids[4]).kind, NodeKind::Modified(_)),
        "the highlight Block is on the selection=2 row, not a different row"
    );
}

#[test]
fn status_suffixes_are_green() {
    let (tree, root) = laid_out();
    let t = all_texts(&tree, root);
    // Active backend "b1" carries [Active]; the subagent Disabled leader (no
    // override set) also carries [Active] — both green.
    let active_green = t.iter().filter(|(text, c)| text == "Active" && *c == GREEN).count();
    assert_eq!(
        active_green, 2,
        "exactly two green [Active] runs: active backend b1 + the subagent Disabled leader (no override)"
    );
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
        NodeKind::Modified(m) => assert!(
            m.ops.iter().any(|o| matches!(o, Mod::Shadow(_))),
            "popup box has a drop shadow"
        ),
        other => panic!("root is the popup box Block, got {other:?}"),
    }
}
