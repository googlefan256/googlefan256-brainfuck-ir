use anyhow::{Context, Result};
use clap::Parser;
use std::{
    fs,
    io::{BufReader, Read},
};

use crate::{
    args::Cli,
    llvm::{compile, link_executable},
    parser::parse_brainfuck,
};
mod args;
mod llvm;
mod parser;

fn main() -> Result<()> {
    let cli = Cli::parse();

    let source_stream = BufReader::new(
        fs::File::open(&cli.input)
            .with_context(|| format!("failed to open input file: {}", cli.input.display()))?,
    );
    let ops = parse_brainfuck(source_stream.bytes().filter_map(Result::ok))?;

    let obj_buf = compile(&ops, &cli.opt, cli.run, cli.output.is_some())?;
    if let (Some(output), Some(obj_buf)) = (&cli.output, &obj_buf) {
        let object_path = output.with_extension("o");
        fs::write(&object_path, obj_buf)?;
        link_executable(&object_path, output, &cli.opt)?;
        if !cli.emit_obj {
            let _ = fs::remove_file(&object_path);
        }
    }
    Ok(())
}
