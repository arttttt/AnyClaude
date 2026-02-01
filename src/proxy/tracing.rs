use tracing_subscriber::EnvFilter;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt};

/// Initialize tracing with optional file output.
///
/// Logging is disabled by default for TUI mode.
/// Set `CLAUDE_WRAPPER_LOG` env var to a file path to enable logging.
pub fn init_tracing() {
    let Some(log_path) = std::env::var("CLAUDE_WRAPPER_LOG").ok() else {
        // No logging configured - skip initialization entirely
        // This is the default for TUI mode to avoid corrupting the display
        return;
    };

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let Ok(file) = std::fs::File::create(&log_path) else {
        eprintln!("Warning: Failed to create log file: {}", log_path);
        return;
    };

    let file_layer = fmt::layer()
        .with_writer(file)
        .with_ansi(false)
        .with_target(true)
        .with_level(true);

    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .init();
}
