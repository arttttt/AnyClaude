use std::io;

fn main() -> io::Result<()> {
    // Note: We intentionally don't call init_tracing() here.
    // Tracing to stdout corrupts the TUI display (logs appear in header area).
    // The proxy runs without console logging when in TUI mode.
    claudewrapper::ui::run()
}
