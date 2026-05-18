use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use std::{
    fs,
    io::{BufReader, Read},
    path::PathBuf,
};

use fastbf::{OptLevel as BOptLevel, compile, link_executable, parse_brainfuck};

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

impl From<OptLevel> for BOptLevel {
    fn from(val: OptLevel) -> Self {
        match val {
            OptLevel::O0 => BOptLevel::O0,
            OptLevel::O1 => BOptLevel::O1,
            OptLevel::O2 => BOptLevel::O2,
            OptLevel::O3 => BOptLevel::O3,
        }
    }
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
    #[arg(default_value = "cc", env = "CC")]
    pub cc: String,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let source_stream = BufReader::new(
        fs::File::open(&cli.input)
            .with_context(|| format!("failed to open input file: {}", cli.input.display()))?,
    );
    let ops = parse_brainfuck(source_stream.bytes().filter_map(Result::ok))?;
    let opt = cli.opt.into();
    let obj_buf = compile(&ops, &opt, cli.run, cli.output.is_some())?;
    if let (Some(output), Some(obj_buf)) = (&cli.output, &obj_buf) {
        let object_path = output.with_extension("o");
        fs::write(&object_path, obj_buf)?;
        link_executable(&cli.cc, &object_path, output, &opt)?;
        if !cli.emit_obj {
            let _ = fs::remove_file(&object_path);
        }
    }
    Ok(())
}
