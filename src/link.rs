use std::path::Path;

use crate::OptLevel;

use anyhow::Context;
use std::process::Command;

pub fn link_executable(
    cc_program: &str,
    object_path: &Path,
    output_path: &Path,
    opt_level: &OptLevel,
) -> anyhow::Result<()> {
    let status = Command::new(cc_program)
        .arg(object_path)
        .arg("-o")
        .arg(output_path)
        .arg(opt_level.cc_opt_flag())
        .status()
        .context("failed to invoke system C compiler (cc)")?;

    if !status.success() {
        anyhow::bail!("linker failed with status {status}");
    }

    Ok(())
}
