use clap::Parser;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use std::io::{self, IsTerminal};

use anyclaude::config::Config;

#[derive(Parser)]
#[command(name = "anyclaude")]
#[command(about = "TUI wrapper for Claude Code with multi-backend support")]
struct Cli {
    /// Override default backend (see config for available backends)
    #[arg(long, value_name = "NAME")]
    backend: Option<String>,

    /// Arguments passed to claude
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

fn main() -> io::Result<()> {
    // Enter raw mode IMMEDIATELY to capture any early input from tmux send-keys.
    // Without this, input arriving before setup_terminal() is lost in cooked mode.
    // Only do this if stdin is a terminal (tests run without TTY).
    let is_tty = io::stdin().is_terminal();
    if is_tty {
        enable_raw_mode()?;
    }

    // Run main logic, ensuring raw mode is disabled on any exit path
    let result = run_main();

    // Always disable raw mode before exiting (guard handles it for normal path,
    // but we need this for error paths before guard is created)
    if is_tty && result.is_err() {
        let _ = disable_raw_mode();
    }

    result
}

fn run_main() -> io::Result<()> {
    let cli = Cli::parse();

    // Load config to validate backend
    let config = Config::load().unwrap_or_default();

    if let Some(ref backend_name) = cli.backend {
        let exists = config.backends.iter().any(|b| &b.name == backend_name);
        if !exists {
            // Must exit raw mode before printing errors
            let _ = disable_raw_mode();
            let available: Vec<_> = config.backends.iter().map(|b| b.name.as_str()).collect();
            eprintln!("Error: Backend '{}' not found in config", backend_name);
            if available.is_empty() {
                eprintln!("No backends configured");
            } else {
                eprintln!("Available backends: {}", available.join(", "));
            }
            std::process::exit(1);
        }
    }

    anyclaude::ui::run(cli.backend, cli.args)
}
