mod event;
mod parser;
mod reader;

pub use event::{Direction, InputEvent, KeyInput, KeyKind, MouseButton, MouseEvent, NavKey};
pub use parser::InputParser;
pub use reader::TtyReader;
