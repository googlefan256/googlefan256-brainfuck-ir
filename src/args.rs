use clap::{Parser, ValueEnum};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum OptLevel {
    #[value(name = "0")]
    O0,
    #[value(name = "1")]
    O1,
    #[value(name = "2")]
    O2,
    #[value(name = "3")]
    O3,
}

#[derive(Parser)]
#[command(author, version, about = "AOT brainfuck compiler using LLVM C API")]
pub struct Cli {
    /// Input brainfuck source file
    pub input: PathBuf,

    /// Output native binary path
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Emit temporary object file
    #[arg(long)]
    pub emit_obj: bool,
    // optimize args
    #[arg(short = 'O', default_value = "O0")]
    pub opt: OptLevel,
    /// Run binary
    #[arg(long)]
    pub run: bool,
}
