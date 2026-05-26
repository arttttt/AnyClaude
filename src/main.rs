use clap::Parser;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use std::io::{self, IsTerminal};

use anyclaude::config::Config;

#[derive(Parser)]
#[command(name = "anyclaude", version)]
#[command(about = "TUI wrapper for Claude Code with multi-backend support")]
struct Cli {
    /// Override default backend (see config for available backends)
    #[arg(long, value_name = "NAME")]
    backend: Option<String>,

    /// Use the GPU-based UI (Phase 5 work in progress). Removed in
    /// the C10 cutover when the GPU UI becomes the default.
    #[arg(long, hide = true)]
    gpu: bool,

    /// Arguments passed to claude
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();

    // GPU path uses winit, which manages its own input — no raw mode.
    if cli.gpu {
        return run_main_gpu(cli);
    }

    // Legacy path: enter raw mode IMMEDIATELY to capture any early input
    // from tmux send-keys. Without this, input arriving before
    // setup_terminal() is lost in cooked mode. Only do this if stdin
    // is a terminal (tests run without TTY).
    let is_tty = io::stdin().is_terminal();
    if is_tty {
        enable_raw_mode()?;
    }

    // Run main logic, ensuring raw mode is disabled on any exit path
    let result = run_main(cli);

    // Always disable raw mode before exiting (guard handles it for normal path,
    // but we need this for error paths before guard is created)
    if is_tty && result.is_err() {
        let _ = disable_raw_mode();
    }

    result
}

fn run_main(cli: Cli) -> io::Result<()> {
    // Load config — fail fast on invalid config
    let config = match Config::load() {
        Ok(config) => config,
        Err(e) => {
            let _ = disable_raw_mode();
            eprintln!("Error: Failed to load config: {}", e);
            eprintln!("Config file: {}", Config::config_path().display());
            std::process::exit(1);
        }
    };

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

fn run_main_gpu(cli: Cli) -> io::Result<()> {
    let config = match Config::load() {
        Ok(config) => config,
        Err(e) => {
            eprintln!("Error: Failed to load config: {}", e);
            eprintln!("Config file: {}", Config::config_path().display());
            std::process::exit(1);
        }
    };

    if let Some(ref backend_name) = cli.backend {
        let exists = config.backends.iter().any(|b| &b.name == backend_name);
        if !exists {
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

    anyclaude::ui::gpu::run(cli.backend, cli.args)
}
