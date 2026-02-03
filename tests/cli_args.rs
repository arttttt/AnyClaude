//! Tests for CLI argument parsing.
//!
//! Note: These tests verify the CLI parsing behavior using the assert_cmd crate
//! to test the actual binary execution.

use std::process::Command;

fn anyclaude_cmd() -> Command {
    Command::new(env!("CARGO_BIN_EXE_anyclaude"))
}

#[test]
fn test_help_shows_backend_option() {
    let output = anyclaude_cmd()
        .arg("--help")
        .output()
        .expect("Failed to execute command");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--backend"));
    assert!(stdout.contains("Override default backend"));
}

#[test]
fn test_invalid_backend_exits_with_error() {
    let output = anyclaude_cmd()
        .arg("--backend")
        .arg("nonexistent_backend_xyz")
        .output()
        .expect("Failed to execute command");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(1));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Error: Backend 'nonexistent_backend_xyz' not found in config"));
}

#[test]
fn test_invalid_backend_shows_available_backends() {
    let output = anyclaude_cmd()
        .arg("--backend")
        .arg("nonexistent")
        .output()
        .expect("Failed to execute command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    // Should show available backends (from default config or user config)
    assert!(
        stderr.contains("Available backends:") || stderr.contains("No backends configured"),
        "Expected available backends message, got: {}",
        stderr
    );
}

#[test]
fn test_missing_backend_value_shows_error() {
    let output = anyclaude_cmd()
        .arg("--backend")
        .output()
        .expect("Failed to execute command");

    // clap should show an error about missing value
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("a value is required") || stderr.contains("requires a value"),
        "Expected clap error about missing value, got: {}",
        stderr
    );
}
