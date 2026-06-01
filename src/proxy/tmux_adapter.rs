//! `TmuxAdapter` — the anti-corruption layer that turns the tmux verbs Claude
//! Code issues into tmux-AGNOSTIC [`TmuxAction`]s. Pure parsing: no HTTP, no UI,
//! no async — the [`handle_tmux`](super::tmux_api::handle_tmux) handler maps the
//! action to a [`ChildSessionEvent`](crate::ui::child_session::ChildSessionEvent)
//! + reply. anyclaude is the layout authority (no real tmux), so geometry verbs
//! are accepted-but-ignored and an unrecognised verb is an explicit error — no
//! silent forwarding.

use crate::ui::child_session::{ChildSpec, PaneId};

/// Default accent for a freshly split pane until `select-pane -P` styles it.
const DEFAULT_ACCENT: [f32; 4] = [0.30, 0.55, 0.95, 1.0];

/// A parsed tmux invocation reduced to what anyclaude acts on.
#[derive(Debug, Clone, PartialEq)]
pub enum TmuxAction {
    /// `split-window [-P] …` — create a pane (a live shell; the teammate command
    /// arrives later via `send-keys`). Reply its `%N`.
    NewPane(ChildSpec),
    /// `kill-pane -t %N` — remove a pane.
    KillPane(PaneId),
    /// `send-keys -t %N <keys…>` — type into a pane's PTY (the teammate command
    /// + Enter). `data` is the already-encoded byte stream.
    SendKeys { pane: PaneId, data: Vec<u8> },
    /// `select-pane -t %N -T <title>` — retitle a pane.
    SetTitle { pane: PaneId, title: String },
    /// Geometry / option verbs accepted but not acted on — anyclaude owns the
    /// layout (`resize-pane`, `select-layout`, `set`/`set-option`, `select-pane`
    /// without `-T`, session bookkeeping). Returns success, no UI change.
    Ack,
    /// A registry query we don't answer with real data yet (`list-panes`,
    /// `display-message`) — logged so a real CC run reveals what's needed.
    Query(String),
    /// An unrecognised verb — surfaced as an error (no silent forwarding).
    Unknown(String),
}

/// Parse a tmux argv (`args[0]` is the verb, as the shim forwards `"$@"`).
pub fn parse(args: &[String]) -> TmuxAction {
    let Some(verb) = args.first() else {
        return TmuxAction::Unknown(String::new());
    };
    match verb.as_str() {
        "split-window" | "splitw" => TmuxAction::NewPane(new_pane_spec()),
        "kill-pane" | "killp" => match target_pane(args) {
            Some(pane) => TmuxAction::KillPane(pane),
            None => TmuxAction::Unknown(format!("kill-pane without -t %N: {}", args.join(" "))),
        },
        "send-keys" | "send" => parse_send_keys(args),
        "select-pane" | "selectp" => parse_select_pane(args),
        // Geometry + options + session bookkeeping: accepted, not followed.
        "resize-pane" | "resizep" | "select-layout" | "selectl" | "set" | "set-option"
        | "setw" | "set-window-option" | "set-hook" | "show" | "show-options" | "showw"
        | "show-window-options" | "rename-window" | "renamew" | "new-session" | "new"
        | "has-session" | "kill-server" | "start-server" => TmuxAction::Ack,
        // Registry queries (no real data yet — C2 returns empty, logged).
        "list-panes" | "lsp" | "display-message" | "display" | "displayp" => {
            TmuxAction::Query(args.join(" "))
        }
        other => TmuxAction::Unknown(other.to_string()),
    }
}

/// `send-keys [-l] -t %N <keys…>` — encode the key arguments into the byte
/// stream typed into the pane's PTY. Key names (`Enter`, `C-c`, …) are
/// interpreted unless `-l` (literal) is given; everything else is typed as-is.
fn parse_send_keys(args: &[String]) -> TmuxAction {
    let Some(pane) = target_pane(args) else {
        return TmuxAction::Unknown(format!("send-keys without -t %N: {}", args.join(" ")));
    };
    let literal = args.iter().any(|a| a == "-l");
    let mut data = Vec::new();
    for key in key_args(args) {
        if literal {
            data.extend_from_slice(key.as_bytes());
        } else {
            data.extend_from_slice(&encode_key(&key));
        }
    }
    TmuxAction::SendKeys { pane, data }
}

/// The key arguments of a `send-keys`: every argv element that isn't the verb,
/// a flag, or a flag's value. Skips `-t <target>` / `-N <count>` (flag + value)
/// and bare short flags (`-l`, `-R`, `-M`, an unmodelled `-X` copy command, …).
fn key_args(args: &[String]) -> Vec<String> {
    let mut keys = Vec::new();
    let mut i = 1; // skip the verb
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "-t" | "-N" => i += 2, // flag with a value
            _ if a.starts_with('-') && a.len() > 1 => i += 1, // bare flag (incl. -t%0 inline)
            _ => {
                keys.push(a.clone());
                i += 1;
            }
        }
    }
    keys
}

/// Encode one `send-keys` token: a tmux key name → its control byte(s), else the
/// literal UTF-8. Covers the names Claude Code uses to drive a teammate shell
/// (`Enter` to run the command, the common control chords); arrows / function
/// keys aren't needed for spawning and fall through to literal.
fn encode_key(token: &str) -> Vec<u8> {
    match token {
        "Enter" | "C-m" | "KPEnter" => vec![b'\r'],
        "Space" => vec![b' '],
        "Tab" | "C-i" => vec![b'\t'],
        "Escape" | "C-[" => vec![0x1b],
        "BSpace" | "C-?" => vec![0x7f],
        _ => {
            // `C-<letter>` → the control byte (letter & 0x1f).
            if let Some(rest) = token.strip_prefix("C-") {
                if rest.len() == 1 {
                    if let Some(c) = rest.chars().next().filter(|c| c.is_ascii_alphabetic()) {
                        return vec![(c.to_ascii_uppercase() as u8) & 0x1f];
                    }
                }
            }
            token.as_bytes().to_vec()
        }
    }
}

/// `select-pane`: `-T <title>` retitles; anything else (focus, `-P` style) is a
/// no-op we accept (anyclaude drives focus from the user, not tmux).
fn parse_select_pane(args: &[String]) -> TmuxAction {
    match (flag_value(args, "-T"), target_pane(args)) {
        (Some(title), Some(pane)) => TmuxAction::SetTitle { pane, title },
        _ => TmuxAction::Ack,
    }
}

/// The pane id from `-t %N` (`-t %0` or `-t%0`). `None` if absent / not a pane.
fn target_pane(args: &[String]) -> Option<PaneId> {
    flag_value(args, "-t").as_deref().and_then(parse_pane_id)
}

/// A pane id token: `%N` → `PaneId(N)`. Rejects window/session targets.
fn parse_pane_id(token: &str) -> Option<PaneId> {
    token.strip_prefix('%')?.parse::<u64>().ok().map(PaneId)
}

/// The value following `flag` — either the next argv element (`-t %0`) or the
/// inline remainder (`-t%0`). `None` when the flag is absent or trailing.
fn flag_value(args: &[String], flag: &str) -> Option<String> {
    for (i, a) in args.iter().enumerate() {
        if a == flag {
            return args.get(i + 1).cloned();
        }
        if let Some(rest) = a.strip_prefix(flag) {
            if !rest.is_empty() {
                return Some(rest.to_string());
            }
        }
    }
    None
}

/// The placeholder spec a `split-window` pane spawns with — a live shell, named
/// generically until `select-pane -T` / the teammate identifies itself.
fn new_pane_spec() -> ChildSpec {
    ChildSpec {
        name: "teammate".to_string(),
        accent: DEFAULT_ACCENT,
        command: std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string()),
        args: vec![],
        env: vec![("TERM".to_string(), "xterm-256color".to_string())],
    }
}
