//! Tests for the teammate PATH shim (tmux).

mod common;

use std::path::Path;

use anyclaude::shim::TeammateShim;

fn shim_dir(shim: &TeammateShim) -> String {
    shim.path_env().1.split(':').next().unwrap().to_string()
}

// ── TeammateShim::create ─────────────────────────────────────────────

#[test]
fn create_succeeds_or_returns_error() {
    // In CI/dev environments claude may or may not be installed.
    // Just verify the function doesn't panic.
    let _ = TeammateShim::create(12345, true);
}

// ── PATH env ─────────────────────────────────────────────────────────

#[test]
fn path_env_prepends_shim_dir() {
    let shim = match TeammateShim::create(12345, true) {
        Ok(s) => s,
        Err(_) => return,
    };

    let (key, value) = shim.path_env();
    assert_eq!(key, "PATH");
    assert!(value.contains(':'), "PATH should contain separator");
    let first_dir = value.split(':').next().unwrap();
    assert!(Path::new(first_dir).exists(), "shim directory should exist");
}

// ── tmux shim ────────────────────────────────────────────────────────

#[test]
fn tmux_shim_exists() {
    let shim = match TeammateShim::create(12345, true) {
        Ok(s) => s,
        Err(_) => return,
    };
    let dir = shim_dir(&shim);
    assert!(Path::new(&dir).join("tmux").exists());
}

#[test]
fn tmux_shim_is_executable() {
    let shim = match TeammateShim::create(12345, true) {
        Ok(s) => s,
        Err(_) => return,
    };
    let dir = shim_dir(&shim);
    let tmux_path = Path::new(&dir).join("tmux");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&tmux_path).unwrap().permissions().mode();
        assert!(mode & 0o111 != 0, "tmux shim should be executable");
    }
}

#[test]
fn tmux_shim_contains_log_and_shim_dir() {
    let shim = match TeammateShim::create(12345, true) {
        Ok(s) => s,
        Err(_) => return,
    };
    let dir = shim_dir(&shim);
    let script = std::fs::read_to_string(Path::new(&dir).join("tmux")).unwrap();

    assert!(script.contains("tmux_shim.log"), "should log to tmux_shim.log");
    assert!(script.contains("SHIM_DIR"), "should reference SHIM_DIR");
    assert!(script.contains("find_real_tmux"), "should have real tmux lookup");
}

#[test]
fn tmux_shim_contains_port_and_injection_logic() {
    let shim = match TeammateShim::create(7777, true) {
        Ok(s) => s,
        Err(_) => return,
    };
    let dir = shim_dir(&shim);
    let script = std::fs::read_to_string(Path::new(&dir).join("tmux")).unwrap();

    assert!(
        script.contains("127.0.0.1:7777/teammate"),
        "should contain proxy port in ANTHROPIC_BASE_URL"
    );
    assert!(
        script.contains("send-keys"),
        "should detect send-keys subcommand"
    );
    assert!(
        script.contains("ANTHROPIC_BASE_URL"),
        "should inject ANTHROPIC_BASE_URL"
    );
}

#[test]
fn tmux_log_path_points_to_shim_dir() {
    let shim = match TeammateShim::create(12345, true) {
        Ok(s) => s,
        Err(_) => return,
    };
    let log_path = shim.tmux_log_path();
    let dir = shim_dir(&shim);
    assert_eq!(
        log_path.parent().unwrap().to_str().unwrap(),
        dir,
        "tmux log should be inside shim dir"
    );
    assert!(
        log_path.to_str().unwrap().ends_with("tmux_shim.log"),
        "log file should be tmux_shim.log"
    );
}
