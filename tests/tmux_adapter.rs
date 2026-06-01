//! Unit tests for the tmux verb parser (the M3 control-plane anti-corruption
//! layer). Pure — no HTTP, no UI — so it exercises `parse` directly.

use anyclaude::proxy::tmux_adapter::{parse, TmuxAction};
use anyclaude::ui::child_session::PaneId;

/// Build an argv from string literals.
fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

#[test]
fn split_window_creates_a_pane_with_a_real_command() {
    match parse(&argv(&["split-window", "-P", "-t", "%0"])) {
        TmuxAction::NewPane(spec) => {
            assert!(!spec.command.is_empty(), "pane should spawn a real command");
        }
        other => panic!("expected NewPane, got {other:?}"),
    }
    // Short alias.
    assert!(matches!(parse(&argv(&["splitw"])), TmuxAction::NewPane(_)));
}

#[test]
fn kill_pane_targets_its_pane_id() {
    assert_eq!(parse(&argv(&["kill-pane", "-t", "%3"])), TmuxAction::KillPane(PaneId(3)));
    // Short alias + inline target form (`-t%5`).
    assert_eq!(parse(&argv(&["killp", "-t%5"])), TmuxAction::KillPane(PaneId(5)));
}

#[test]
fn kill_pane_without_a_target_is_unknown() {
    assert!(matches!(parse(&argv(&["kill-pane"])), TmuxAction::Unknown(_)));
}

#[test]
fn send_keys_encodes_literal_text_plus_enter() {
    // The canonical teammate spawn: type a command, then Enter (→ CR).
    match parse(&argv(&["send-keys", "-t", "%0", "echo hi", "Enter"])) {
        TmuxAction::SendKeys { pane, data } => {
            assert_eq!(pane, PaneId(0));
            assert_eq!(data, b"echo hi\r");
        }
        other => panic!("expected SendKeys, got {other:?}"),
    }
}

#[test]
fn send_keys_literal_flag_does_not_interpret_key_names() {
    match parse(&argv(&["send-keys", "-t", "%1", "-l", "type Enter please"])) {
        TmuxAction::SendKeys { pane, data } => {
            assert_eq!(pane, PaneId(1));
            // `-l`: "Enter" inside the string stays literal, not a CR.
            assert_eq!(data, b"type Enter please");
        }
        other => panic!("expected SendKeys, got {other:?}"),
    }
}

#[test]
fn send_keys_encodes_control_chords() {
    assert_eq!(
        parse(&argv(&["send-keys", "-t", "%2", "C-c"])),
        TmuxAction::SendKeys { pane: PaneId(2), data: vec![0x03] },
    );
    assert_eq!(
        parse(&argv(&["send-keys", "-t", "%2", "Escape"])),
        TmuxAction::SendKeys { pane: PaneId(2), data: vec![0x1b] },
    );
}

#[test]
fn send_keys_without_a_target_is_unknown() {
    assert!(matches!(parse(&argv(&["send-keys", "echo hi", "Enter"])), TmuxAction::Unknown(_)));
}

#[test]
fn select_pane_dash_t_sets_the_title() {
    assert_eq!(
        parse(&argv(&["select-pane", "-t", "%2", "-T", "module-mapper"])),
        TmuxAction::SetTitle { pane: PaneId(2), title: "module-mapper".to_string() },
    );
}

#[test]
fn select_pane_without_title_is_accepted_but_ignored() {
    // Plain focus / style — anyclaude drives focus from the user, not tmux.
    assert_eq!(parse(&argv(&["select-pane", "-t", "%2"])), TmuxAction::Ack);
    assert_eq!(parse(&argv(&["select-pane", "-t", "%2", "-P", "bg=red"])), TmuxAction::Ack);
}

#[test]
fn geometry_and_option_verbs_are_acked() {
    for v in ["resize-pane", "select-layout", "set", "set-option", "new-session", "has-session"] {
        assert_eq!(parse(&argv(&[v])), TmuxAction::Ack, "{v} should ack");
    }
}

#[test]
fn queries_are_flagged_for_later() {
    assert!(matches!(parse(&argv(&["list-panes", "-F", "#{pane_id}"])), TmuxAction::Query(_)));
    assert!(matches!(parse(&argv(&["display-message", "-p", "#{pane_id}"])), TmuxAction::Query(_)));
}

#[test]
fn unknown_verb_and_empty_argv_are_errors() {
    assert!(matches!(parse(&argv(&["frobnicate"])), TmuxAction::Unknown(_)));
    assert!(matches!(parse(&argv(&[])), TmuxAction::Unknown(_)));
}

#[test]
fn pane_id_must_be_a_percent_target() {
    // A window target (`@0`) or bare number is not a pane id → no kill target.
    assert!(matches!(parse(&argv(&["kill-pane", "-t", "@0"])), TmuxAction::Unknown(_)));
}
