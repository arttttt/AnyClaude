use clap::Parser;
use std::io;

use claudewrapper::config::Config;

#[derive(Parser)]
#[command(name = "claudewrapper")]
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
    // Note: We intentionally don't call init_tracing() here.
    // Tracing to stdout corrupts the TUI display (logs appear in header area).
    // The proxy runs without console logging when in TUI mode.

    let cli = Cli::parse();

    // Load config to validate backend
    let config = Config::load().unwrap_or_default();

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

    claudewrapper::ui::run(cli.backend, cli.args)
}
