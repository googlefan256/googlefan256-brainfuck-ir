use anyhow::{bail, Context, Result};
use clap::Parser;
use std::{fs, process::Command};

use crate::{
    llvm::{compile_to_object, link_executable, Cli, OptLevel},
    parser::parse_brainfuck,
};
mod llvm;
mod parser;
fn main() -> Result<()> {
    let cli = Cli::parse();

    let source = fs::read_to_string(&cli.input)
        .with_context(|| format!("failed to read input file: {}", cli.input.display()))?;
    let ops = parse_brainfuck(&source)?;

    let object_path = cli.output.with_extension("o");
    let opt = cli.opt.unwrap_or(OptLevel::O0);
    unsafe { compile_to_object(&ops, &object_path, &opt)? };
    let executable_path = link_executable(&object_path, &cli.output, &opt)?;

    if !cli.keep_obj {
        let _ = fs::remove_file(&object_path);
    }

    if cli.run {
        let full_path = fs::canonicalize(&executable_path).with_context(|| {
            format!(
                "failed to canonicalize executable path: {}",
                executable_path.display()
            )
        })?;
        let status = Command::new(&full_path)
            .status()
            .context("failed to run generated executable")?;
        if !status.success() {
            bail!("generated executable failed with status {status}");
        }
    }

    Ok(())
}
