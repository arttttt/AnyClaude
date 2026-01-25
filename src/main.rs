use claudewrapper::pty::{parse_command, PtyManager};
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let (command, args) = parse_command();
    let mut pty_manager = PtyManager::new();
    pty_manager.run_command(command, args)
}
