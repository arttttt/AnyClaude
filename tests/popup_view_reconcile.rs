//! Reconcile-path coverage for the popup second tree (E.7 review L6/L7). The
//! live coordinator runs `reconcile_root` on ONE retained tree every
//! steady-state frame — and reuses that tree across popup KINDS (history →
//! settings → backend, all `Block`s). The per-popup integration suites only
//! `build_root` on a fresh tree, so this file exercises the reconcile path:
//! an in-place field toggle, and a cross-kind A→B→C reconcile. Headless.

use std::time::{Instant, UNIX_EPOCH};

use anyclaude::config::{SettingId, SettingSection, SettingsFieldSnapshot};
use anyclaude::ui::app_state::AppState;
use anyclaude::ui::backend_switch::{BackendPopupSection, BackendSwitchState};
use anyclaude::ui::history::{HistoryDialogState, HistoryEntry};
use anyclaude::ui::popup_view::{backend_view, popup_view, POPUP_MIN_WIDTH};
use anyclaude::ui::settings::{SettingsDialogState, SettingsIntent};
use glam::Vec2;
use term_gpu::{FontFamily, FontSystem, TextShapeCache};
use term_ui::{
    build_root, measure, place_centered, reconcile_root, NodeId, NodeKind, RetainedTree,
    SizeConstraint,
};

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

fn app_with_settings(fields: Vec<SettingsFieldSnapshot>) -> AppState {
    let mut state = AppState::new("sid".to_string(), Instant::now(), (80, 24));
    state.settings = SettingsDialogState::Visible { fields, focused: 0, dirty: false, confirm_discard: false };
    state
}

fn app_with_history(n: usize) -> AppState {
    let mut state = AppState::new("sid".to_string(), Instant::now(), (80, 24));
    let entries = (0..n)
        .map(|i| HistoryEntry { timestamp: UNIX_EPOCH, from_backend: None, to_backend: format!("b{i}") })
        .collect::<Vec<_>>();
    state.history = HistoryDialogState::Visible { entries, scroll_offset: 0 };
    state
}

fn backend_state() -> BackendSwitchState {
    BackendSwitchState::Visible {
        section: BackendPopupSection::ActiveBackend,
        backend_selection: 0,
        subagent_selection: 0,
        teammate_selection: 0,
        backends_count: 2,
    }
}

/// Recursively collect every Text node's string under `node`.
fn texts(tree: &RetainedTree, node: NodeId, out: &mut Vec<String>) {
    if let NodeKind::Text(s) = &tree.node(node).kind {
        out.push(s.text.clone());
    }
    for c in tree.node(node).children.clone() {
        texts(tree, c, out);
    }
}

fn all_texts(tree: &RetainedTree, root: NodeId) -> Vec<String> {
    let mut v = Vec::new();
    texts(tree, root, &mut v);
    v
}

fn assert_well_formed(
    tree: &mut RetainedTree,
    root: NodeId,
    fonts: &mut FontSystem,
    shape: &mut TextShapeCache,
    label: &str,
) {
    assert!(tree.is_live(root), "{label}: root still live after reconcile");
    measure(
        tree,
        root,
        SizeConstraint::new(Vec2::new(POPUP_MIN_WIDTH, 0.0), VIEWPORT),
        fonts,
        shape,
        1.0,
    );
    place_centered(tree, root, VIEWPORT);
    assert!(matches!(tree.node(root).kind, NodeKind::Block(_)), "{label}: root is a Block");
    assert!(tree.node(root).measured.x >= POPUP_MIN_WIDTH - 0.5, "{label}: min-width floored");
}

#[test]
fn settings_toggle_reconciles_in_place() {
    // Build from an unchecked field, then reconcile against the toggled view on
    // the SAME tree/root (the live per-frame path), and assert the checkbox
    // glyph flipped in place — not via a fresh rebuild.
    let mut state = app_with_settings(vec![field("Agent Teams", false)]);
    let v0 = popup_view(&state).unwrap();
    let mut tree = RetainedTree::new();
    let root = build_root(&mut tree, &v0);
    assert!(all_texts(&tree, root).iter().any(|t| t.starts_with("[ ]")), "starts unchecked");

    state.settings.apply(SettingsIntent::Toggle);
    let v1 = popup_view(&state).unwrap();
    reconcile_root(&mut tree, root, &v0, &v1);

    let after = all_texts(&tree, root);
    assert!(after.iter().any(|t| t.starts_with("[x]")), "checkbox flipped to [x] in place");
    assert!(!after.iter().any(|t| t.starts_with("[ ]")), "no unchecked row remains");
}

#[test]
fn one_tree_reconciles_across_all_three_popup_kinds() {
    // The coordinator reuses ONE retained tree across popup kinds; each view is
    // a Block, so reconcile_root diffs them and the splice rebuilds the differing
    // bodies. The tree must stay live + well-formed at each hop.
    let mut fonts = FontSystem::new();
    let mut shape = TextShapeCache::with_family(FontFamily::SansSerif);

    let hist = app_with_history(5);
    let set = app_with_settings(vec![field("A", true)]);

    let v_hist = popup_view(&hist).unwrap();
    let mut tree = RetainedTree::new();
    let root = build_root(&mut tree, &v_hist);
    assert_well_formed(&mut tree, root, &mut fonts, &mut shape, "history (build)");

    let v_set = popup_view(&set).unwrap();
    reconcile_root(&mut tree, root, &v_hist, &v_set);
    assert_well_formed(&mut tree, root, &mut fonts, &mut shape, "history->settings");
    assert!(all_texts(&tree, root).iter().any(|t| t.contains("Space toggle")), "settings title present");

    let v_back = backend_view(&backend_state(), &[("B0".into(), "b0".into()), ("B1".into(), "b1".into())], "b0", None, None);
    reconcile_root(&mut tree, root, &v_set, &v_back);
    assert_well_formed(&mut tree, root, &mut fonts, &mut shape, "settings->backend");
    assert!(all_texts(&tree, root).iter().any(|t| t == "Select Backend"), "backend title present");
}
