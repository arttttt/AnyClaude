use clap::Parser;
use std::io;

use anyclaude::config::Config;

#[derive(Parser)]
#[command(name = "anyclaude", version)]
#[command(about = "GPU TUI wrapper for Claude Code with multi-backend support")]
struct Cli {
    /// Override default backend (see config for available backends)
    #[arg(long, value_name = "NAME")]
    backend: Option<String>,

    /// Arguments passed to claude
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();

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
