use std::io;

fn main() -> io::Result<()> {
    claudewrapper::proxy::init_tracing();
    claudewrapper::ui::run()
}
