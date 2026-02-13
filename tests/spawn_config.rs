mod common;

use anyclaude::pty::{PtySpawnConfig, SessionMode, encode_project_path};

fn config(base_args: Vec<&str>) -> PtySpawnConfig {
    PtySpawnConfig::new(
        "claude".to_string(),
        base_args.into_iter().map(String::from).collect(),
        "http://localhost:3000".to_string(),
    )
}

// -- initial spawn (SessionMode::Initial) -------------------------------------

#[test]
fn initial_adds_session_id() {
    let cfg = config(vec!["--model", "opus"]);
    let p = cfg.build(vec![], vec![], SessionMode::Initial);
    assert_eq!(p.args.len(), 4);
    assert_eq!(p.args[0], "--model");
    assert_eq!(p.args[1], "opus");
    assert_eq!(p.args[2], "--session-id");
    assert_eq!(p.args[3], cfg.session_id());
}

#[test]
fn initial_env_contains_base_url() {
    let cfg = config(vec![]);
    let p = cfg.build(vec![], vec![], SessionMode::Initial);
    assert_eq!(p.env.len(), 1);
    assert_eq!(p.env[0].0, "ANTHROPIC_BASE_URL");
    assert_eq!(p.env[0].1, "http://localhost:3000");
}

#[test]
fn initial_session_id_is_valid_uuid() {
    let cfg = config(vec![]);
    assert!(uuid::Uuid::parse_str(cfg.session_id()).is_ok());
}

// -- restart (SessionMode::Resume) --------------------------------------------

#[test]
fn resume_uses_internal_session_id() {
    let cfg = config(vec!["--model", "opus"]);
    let p = cfg.build(vec![], vec![], SessionMode::Resume);
    assert_eq!(
        p.args,
        vec!["--model", "opus", "--resume", cfg.session_id()]
    );
}

#[test]
fn resume_appends_extra_args() {
    let cfg = config(vec!["--model", "opus"]);
    let extra = vec!["--verbose".to_string()];
    let p = cfg.build(vec![], extra, SessionMode::Resume);
    assert!(p.args.contains(&"--verbose".to_string()));
    assert!(p.args.contains(&"--resume".to_string()));
}

#[test]
fn empty_base_args_resume() {
    let cfg = config(vec![]);
    let p = cfg.build(vec![], vec![], SessionMode::Resume);
    assert_eq!(p.args, vec!["--resume", cfg.session_id()]);
}

// -- shared -------------------------------------------------------------------

#[test]
fn restart_merges_extra_env() {
    let cfg = config(vec![]);
    let extra_env = vec![("FOO".to_string(), "1".to_string())];
    let p = cfg.build(extra_env, vec![], SessionMode::Resume);
    assert_eq!(p.env.len(), 2);
    assert_eq!(p.env[0].0, "ANTHROPIC_BASE_URL");
    assert_eq!(p.env[1], ("FOO".to_string(), "1".to_string()));
}

#[test]
fn command_accessor() {
    let cfg = config(vec![]);
    assert_eq!(cfg.command(), "claude");
}

#[test]
fn session_id_stable_across_builds() {
    let cfg = config(vec![]);
    let p1 = cfg.build(vec![], vec![], SessionMode::Initial);
    let p2 = cfg.build(vec![], vec![], SessionMode::Resume);
    let initial_id = &p1.args[p1.args.len() - 1];
    let resume_id = &p2.args[p2.args.len() - 1];
    assert_eq!(initial_id, resume_id);
    assert_eq!(initial_id, cfg.session_id());
}

#[test]
fn different_configs_get_different_ids() {
    let cfg1 = config(vec![]);
    let cfg2 = config(vec![]);
    assert_ne!(cfg1.session_id(), cfg2.session_id());
}

// -- user-provided session flags adopted in new() -----------------------------

#[test]
fn user_session_id_adopted() {
    let cfg = config(vec!["--session-id", "my-custom-id", "--model", "opus"]);
    assert_eq!(cfg.session_id(), "my-custom-id");
    // Session flag stripped from base_args, other args preserved
    let p = cfg.build(vec![], vec![], SessionMode::Initial);
    assert_eq!(p.args, vec!["--model", "opus", "--session-id", "my-custom-id"]);
}

#[test]
fn user_resume_id_adopted() {
    let cfg = config(vec!["--resume", "existing-session", "--model", "opus"]);
    assert_eq!(cfg.session_id(), "existing-session");
    let p = cfg.build(vec![], vec![], SessionMode::Initial);
    assert_eq!(p.args, vec!["--model", "opus", "--session-id", "existing-session"]);
}

#[test]
fn user_resume_id_on_restart() {
    let cfg = config(vec!["--resume", "existing-session", "--model", "opus"]);
    let p = cfg.build(vec![], vec![], SessionMode::Resume);
    assert_eq!(p.args, vec!["--model", "opus", "--resume", "existing-session"]);
}

#[test]
fn user_session_id_on_restart() {
    let cfg = config(vec!["--session-id", "my-id", "--model", "opus"]);
    let p = cfg.build(vec![], vec![], SessionMode::Resume);
    assert_eq!(p.args, vec!["--model", "opus", "--resume", "my-id"]);
}

// -- --continue without value preserves next arg ------------------------------

#[test]
fn continue_preserves_next_arg() {
    let cfg = config(vec!["--continue", "--model", "opus"]);
    let p = cfg.build(vec![], vec![], SessionMode::Initial);
    assert!(p.args.contains(&"--model".to_string()));
    assert!(p.args.contains(&"opus".to_string()));
    // --continue itself is stripped
    assert!(!p.args.contains(&"--continue".to_string()));
}

// -- session-id/resume with value strip correctly -----------------------------

#[test]
fn session_id_value_stripped_from_base_args() {
    let cfg = config(vec!["--session-id", "old-id", "--model", "opus"]);
    assert_eq!(cfg.session_id(), "old-id");
    let p = cfg.build(vec![], vec![], SessionMode::Initial);
    assert_eq!(p.args, vec!["--model", "opus", "--session-id", "old-id"]);
}

#[test]
fn resume_value_stripped_from_base_args() {
    let cfg = config(vec!["--resume", "old-id", "--model", "opus"]);
    let p = cfg.build(vec![], vec![], SessionMode::Resume);
    // Only one occurrence of "old-id" — as the value of --resume
    assert_eq!(p.args, vec!["--model", "opus", "--resume", "old-id"]);
}

// -- warnings -----------------------------------------------------------------

#[test]
fn no_warnings_without_session_flags() {
    let cfg = config(vec!["--model", "opus"]);
    assert!(cfg.warnings().is_empty());
}

#[test]
fn no_warnings_with_session_id() {
    let cfg = config(vec!["--session-id", "my-id"]);
    assert!(cfg.warnings().is_empty());
}

#[test]
fn no_warnings_with_resume() {
    let cfg = config(vec!["--resume", "my-id"]);
    assert!(cfg.warnings().is_empty());
}

// -- bare flags without values ------------------------------------------------

#[test]
fn session_id_without_value_generates_uuid() {
    let cfg = config(vec!["--session-id", "--model", "opus"]);
    // --session-id has no value (next arg starts with --), should generate UUID
    assert_ne!(cfg.session_id(), "--model");
    assert!(uuid::Uuid::parse_str(cfg.session_id()).is_ok());
    assert!(!cfg.warnings().is_empty());
    let p = cfg.build(vec![], vec![], SessionMode::Initial);
    assert!(p.args.contains(&"--model".to_string()));
    assert!(p.args.contains(&"opus".to_string()));
}

#[test]
fn resume_without_value_generates_uuid() {
    let cfg = config(vec!["--resume", "--model", "opus"]);
    assert_ne!(cfg.session_id(), "--model");
    assert!(uuid::Uuid::parse_str(cfg.session_id()).is_ok());
    assert!(!cfg.warnings().is_empty());
}

#[test]
fn session_id_as_last_arg_generates_uuid() {
    let cfg = config(vec!["--model", "opus", "--session-id"]);
    assert!(uuid::Uuid::parse_str(cfg.session_id()).is_ok());
    assert!(!cfg.warnings().is_empty());
    let p = cfg.build(vec![], vec![], SessionMode::Initial);
    assert!(p.args.contains(&"--model".to_string()));
    assert!(p.args.contains(&"opus".to_string()));
}

// -- conflicting flags --------------------------------------------------------

#[test]
fn session_id_and_resume_last_wins() {
    let cfg = config(vec!["--session-id", "id-1", "--resume", "id-2"]);
    assert_eq!(cfg.session_id(), "id-2");
    assert!(!cfg.warnings().is_empty());
}

#[test]
fn session_id_and_continue_explicit_wins() {
    let cfg = config(vec!["--session-id", "my-id", "--continue"]);
    assert_eq!(cfg.session_id(), "my-id");
    assert!(!cfg.warnings().is_empty());
}

// -- continue with Resume mode ------------------------------------------------

#[test]
fn continue_preserves_next_arg_on_resume() {
    let cfg = config(vec!["--continue", "--model", "opus"]);
    let p = cfg.build(vec![], vec![], SessionMode::Resume);
    assert!(p.args.contains(&"--model".to_string()));
    assert!(p.args.contains(&"opus".to_string()));
    assert!(!p.args.contains(&"--continue".to_string()));
    assert!(p.args.contains(&"--resume".to_string()));
}

// -- encode_project_path ------------------------------------------------------

#[test]
fn encode_replaces_slashes_with_dashes() {
    assert_eq!(
        encode_project_path("/Users/artem/Projects/Foo"),
        "-Users-artem-Projects-Foo"
    );
}

#[test]
fn encode_root_path() {
    assert_eq!(encode_project_path("/"), "-");
}

#[test]
fn encode_no_slashes() {
    assert_eq!(encode_project_path("plain"), "plain");
}

#[test]
fn encode_empty_string() {
    assert_eq!(encode_project_path(""), "");
}
