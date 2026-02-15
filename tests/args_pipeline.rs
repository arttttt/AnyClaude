//! Integration tests for the args pipeline module.

use anyclaude::args::{
    classify, flag_registry, build_restart_params, build_spawn_params,
    ClassifiedArg, EnvSet, SessionMode,
};
use anyclaude::config::ClaudeSettingsManager;
use anyclaude::pty::encode_project_path;

fn raw_args(args: Vec<&str>) -> Vec<String> {
    args.into_iter().map(String::from).collect()
}

fn build_initial(raw_args: &[String]) -> anyclaude::args::SpawnParams {
    build_spawn_params(
        raw_args,
        SessionMode::Initial,
        "http://localhost:3000",
        &ClaudeSettingsManager::new(),
        None,
    )
}

fn build_resume(raw_args: &[String]) -> anyclaude::args::SpawnParams {
    build_spawn_params(
        raw_args,
        SessionMode::Resume,
        "http://localhost:3000",
        &ClaudeSettingsManager::new(),
        None,
    )
}

// =============================================================================
// CLASSIFIER TESTS
// =============================================================================

#[test]
fn classify_known_long_flag_with_value() {
    let args = raw_args(vec!["--session-id", "abc123", "--verbose"]);
    let result = classify(&args, &flag_registry());

    assert!(matches!(
        &result.args[0],
        ClassifiedArg::Intercepted { flag, value }
        if flag == "--session-id" && value.as_deref() == Some("abc123")
    ));
    assert!(matches!(
        &result.args[1],
        ClassifiedArg::KnownPassthrough { flag, value }
        if flag == "--verbose" && value.is_none()
    ));
}

#[test]
fn classify_short_flag() {
    let args = raw_args(vec!["-r", "abc123"]);
    let result = classify(&args, &flag_registry());

    assert!(matches!(
        &result.args[0],
        ClassifiedArg::Intercepted { flag, value }
        if flag == "--resume" && value.as_deref() == Some("abc123")
    ));
}

#[test]
fn classify_unknown_flag_warns() {
    let args = raw_args(vec!["--typo-flag"]);
    let result = classify(&args, &flag_registry());

    assert!(matches!(
        &result.args[0],
        ClassifiedArg::UnknownPassthrough(s) if s == "--typo-flag"
    ));
    assert!(result.warnings.iter().any(|w| w.contains("unknown flag")));
}

#[test]
fn classify_missing_value() {
    let args = raw_args(vec!["--session-id"]);
    let result = classify(&args, &flag_registry());

    assert!(matches!(
        &result.args[0],
        ClassifiedArg::Intercepted { flag, value }
        if flag == "--session-id" && value.is_none()
    ));
    assert!(result.warnings.iter().any(|w| w.contains("missing required value")));
}

#[test]
fn classify_positional() {
    let args = raw_args(vec!["some-file.txt"]);
    let result = classify(&args, &flag_registry());

    assert!(matches!(
        &result.args[0],
        ClassifiedArg::Positional(s) if s == "some-file.txt"
    ));
}

// =============================================================================
// ENV_BUILDER TESTS
// =============================================================================

#[test]
fn env_set_proxy_url() {
    let env = EnvSet::new()
        .with_proxy_url("http://127.0.0.1:8080")
        .build();

    assert!(env.iter().any(|(k, v)| k == "ANTHROPIC_BASE_URL" && v == "http://127.0.0.1:8080"));
}

#[test]
fn env_set_chaining() {
    let env = EnvSet::new()
        .with_proxy_url("http://127.0.0.1:8080")
        .with_extra(vec![("CUSTOM_VAR".into(), "value".into())])
        .build();

    assert_eq!(env.len(), 2);
}

// =============================================================================
// PIPELINE TESTS
// =============================================================================

// -- initial spawn (SessionMode::Initial) -------------------------------------

#[test]
fn initial_adds_session_id() {
    let args = raw_args(vec!["--model", "opus"]);
    let p = build_initial(&args);
    assert!(p.args.contains(&"--session-id".to_string()));
    assert!(p.args.contains(&"--model".to_string()));
    assert!(p.args.contains(&"opus".to_string()));
}

#[test]
fn initial_env_contains_base_url() {
    let args = raw_args(vec![]);
    let p = build_initial(&args);
    assert!(p.env.iter().any(|(k, v)| k == "ANTHROPIC_BASE_URL" && v == "http://localhost:3000"));
}

#[test]
fn initial_session_id_is_valid_uuid() {
    let args = raw_args(vec![]);
    let p = build_initial(&args);
    assert!(uuid::Uuid::parse_str(&p.session_id).is_ok());
}

#[test]
fn initial_command_is_claude() {
    let args = raw_args(vec![]);
    let p = build_initial(&args);
    assert_eq!(p.command, "claude");
}

// -- restart (SessionMode::Resume) --------------------------------------------

#[test]
fn resume_uses_session_id() {
    let args = raw_args(vec!["--model", "opus"]);
    let p = build_resume(&args);
    assert!(p.args.contains(&"--resume".to_string()));
    assert!(p.args.contains(&"--model".to_string()));
    assert!(p.args.contains(&"opus".to_string()));
}

#[test]
fn resume_appends_extra_args() {
    let args = raw_args(vec!["--model", "opus"]);
    let extra = vec!["--verbose".to_string()];
    let p = build_restart_params(
        &args,
        SessionMode::Resume,
        "http://localhost:3000",
        &ClaudeSettingsManager::new(),
        None,
        vec![],
        extra,
    );
    assert!(p.args.contains(&"--verbose".to_string()));
    assert!(p.args.contains(&"--resume".to_string()));
}

#[test]
fn empty_base_args_resume() {
    let args = raw_args(vec![]);
    let p = build_resume(&args);
    assert!(p.args.contains(&"--resume".to_string()));
}

// -- extra env/args merging ---------------------------------------------------

#[test]
fn restart_merges_extra_env() {
    let args = raw_args(vec![]);
    let extra_env = vec![("FOO".to_string(), "1".to_string())];
    let p = build_restart_params(
        &args,
        SessionMode::Resume,
        "http://localhost:3000",
        &ClaudeSettingsManager::new(),
        None,
        extra_env,
        vec![],
    );
    assert!(p.env.iter().any(|(k, _)| k == "ANTHROPIC_BASE_URL"));
    assert!(p.env.iter().any(|(k, v)| k == "FOO" && v == "1"));
}

// -- user-provided session flags ---------------------------------------------

#[test]
fn user_session_id_adopted() {
    let args = raw_args(vec!["--session-id", "my-custom-id", "--model", "opus"]);
    let p = build_initial(&args);
    assert_eq!(p.session_id, "my-custom-id");
    assert!(p.args.contains(&"--model".to_string()));
    assert!(p.args.contains(&"opus".to_string()));
}

#[test]
fn user_resume_id_adopted() {
    let args = raw_args(vec!["--resume", "existing-session", "--model", "opus"]);
    let p = build_initial(&args);
    assert_eq!(p.session_id, "existing-session");
    assert!(p.args.contains(&"--model".to_string()));
    assert!(p.args.contains(&"opus".to_string()));
}

#[test]
fn user_resume_id_on_restart() {
    let args = raw_args(vec!["--resume", "existing-session", "--model", "opus"]);
    let p = build_resume(&args);
    assert!(p.args.contains(&"--model".to_string()));
    assert!(p.args.contains(&"opus".to_string()));
    assert!(p.args.contains(&"--resume".to_string()));
}

// -- --continue handling ------------------------------------------------------

#[test]
fn continue_preserves_next_arg() {
    let args = raw_args(vec!["--continue", "--model", "opus"]);
    let p = build_initial(&args);
    assert!(p.args.contains(&"--model".to_string()));
    assert!(p.args.contains(&"opus".to_string()));
    assert!(!p.args.contains(&"--continue".to_string()));
}

#[test]
fn continue_preserves_next_arg_on_resume() {
    let args = raw_args(vec!["--continue", "--model", "opus"]);
    let p = build_resume(&args);
    assert!(p.args.contains(&"--model".to_string()));
    assert!(p.args.contains(&"opus".to_string()));
    assert!(!p.args.contains(&"--continue".to_string()));
    assert!(p.args.contains(&"--resume".to_string()));
}

// -- session-id/resume value stripping ----------------------------------------

#[test]
fn session_id_value_stripped_from_base_args() {
    let args = raw_args(vec!["--session-id", "old-id", "--model", "opus"]);
    let p = build_initial(&args);
    assert_eq!(p.session_id, "old-id");
    assert!(p.args.contains(&"--model".to_string()));
    assert!(p.args.contains(&"opus".to_string()));
}

#[test]
fn resume_value_stripped_from_base_args() {
    let args = raw_args(vec!["--resume", "old-id", "--model", "opus"]);
    let p = build_resume(&args);
    assert!(p.args.contains(&"--model".to_string()));
    assert!(p.args.contains(&"opus".to_string()));
}

// -- warnings ----------------------------------------------------------------

#[test]
fn no_warnings_without_session_flags() {
    let args = raw_args(vec!["--model", "opus"]);
    let p = build_initial(&args);
    assert!(p.warnings.iter().all(|w| !w.contains("session")));
}

#[test]
fn no_warnings_with_session_id() {
    let args = raw_args(vec!["--session-id", "my-id"]);
    let p = build_initial(&args);
    assert!(p.warnings.is_empty());
}

#[test]
fn no_warnings_with_resume() {
    let args = raw_args(vec!["--resume", "my-id"]);
    let p = build_initial(&args);
    assert!(p.warnings.is_empty());
}

// -- bare flags without values ------------------------------------------------

#[test]
fn session_id_without_value_generates_uuid() {
    let args = raw_args(vec!["--session-id", "--model", "opus"]);
    let p = build_initial(&args);
    assert_ne!(p.session_id, "--model");
    assert!(uuid::Uuid::parse_str(&p.session_id).is_ok());
    assert!(!p.warnings.is_empty());
    assert!(p.args.contains(&"--model".to_string()));
    assert!(p.args.contains(&"opus".to_string()));
}

#[test]
fn resume_without_value_generates_uuid() {
    let args = raw_args(vec!["--resume", "--model", "opus"]);
    let p = build_initial(&args);
    assert_ne!(p.session_id, "--model");
    assert!(uuid::Uuid::parse_str(&p.session_id).is_ok());
    assert!(!p.warnings.is_empty());
}

#[test]
fn session_id_as_last_arg_generates_uuid() {
    let args = raw_args(vec!["--model", "opus", "--session-id"]);
    let p = build_initial(&args);
    assert!(uuid::Uuid::parse_str(&p.session_id).is_ok());
    assert!(!p.warnings.is_empty());
    assert!(p.args.contains(&"--model".to_string()));
    assert!(p.args.contains(&"opus".to_string()));
}

// -- conflicting flags --------------------------------------------------------

#[test]
fn session_id_and_resume_last_wins() {
    let args = raw_args(vec!["--session-id", "id-1", "--resume", "id-2"]);
    let p = build_initial(&args);
    assert_eq!(p.session_id, "id-2");
    assert!(!p.warnings.is_empty());
}

#[test]
fn session_id_and_continue_explicit_wins() {
    let args = raw_args(vec!["--session-id", "my-id", "--continue"]);
    let p = build_initial(&args);
    assert_eq!(p.session_id, "my-id");
    assert!(!p.warnings.is_empty());
}

// -- unknown flags ------------------------------------------------------------

#[test]
fn unknown_flag_warning() {
    let args = raw_args(vec!["--unknown-flag"]);
    let p = build_initial(&args);
    assert!(p.warnings.iter().any(|w| w.contains("unknown flag")));
    assert!(p.args.contains(&"--unknown-flag".to_string()));
}

// -- positional args ----------------------------------------------------------

#[test]
fn positional_args_preserved() {
    let args = raw_args(vec!["file1.txt", "file2.txt"]);
    let p = build_initial(&args);
    assert!(p.args.contains(&"file1.txt".to_string()));
    assert!(p.args.contains(&"file2.txt".to_string()));
}

// -- different configs get different ids --------------------------------------

#[test]
fn different_spawns_get_different_ids() {
    let args1 = raw_args(vec![]);
    let args2 = raw_args(vec![]);
    let p1 = build_initial(&args1);
    let p2 = build_initial(&args2);
    assert_ne!(p1.session_id, p2.session_id);
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
