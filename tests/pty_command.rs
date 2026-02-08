mod common;

use anyclaude::pty::parse_command_from;

#[test]
fn parse_command_defaults_to_claude() {
    let (command, args) = parse_command_from(Vec::new());
    assert_eq!(command, "claude");
    assert!(args.is_empty());
}

#[test]
fn parse_command_with_args() {
    let args = vec!["--debug".to_string(), "--model".to_string()];
    let (command, remaining) = parse_command_from(args);
    assert_eq!(command, "claude");
    assert_eq!(
        remaining,
        vec!["--debug".to_string(), "--model".to_string()]
    );
}
