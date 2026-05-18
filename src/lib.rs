#[cfg(feature = "link")]
mod link;
mod llvm;
mod parser;
#[cfg(feature = "link")]
pub use link::link_executable;
pub use llvm::{OptLevel, compile};
pub use parser::{CellUpdate, Op, parse_brainfuck};
