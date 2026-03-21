use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn find_llvm_config() -> anyhow::Result<PathBuf> {
    if let Ok(path) = env::var("LLVM_CONFIG_PATH") {
        let candidate = PathBuf::from(path);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    let default = PathBuf::from("llvm-config");
    if Command::new(&default).arg("--version").output().is_ok() {
        return Ok(default);
    }

    if cfg!(target_os = "macos") {
        if let Ok(output) = Command::new("brew").args(["--prefix", "llvm"]).output() {
            if output.status.success() {
                let prefix = String::from_utf8(output.stdout)?.trim().to_string();
                let candidate = Path::new(&prefix).join("bin").join("llvm-config");
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    }

    anyhow::bail!(
        "could not find llvm-config. Set LLVM_CONFIG_PATH, add llvm-config to PATH, or (on macOS) install LLVM via Homebrew"
    )
}

fn run_llvm_config(llvm_config: &Path, arg: &str) -> anyhow::Result<String> {
    let output = Command::new(llvm_config).arg(arg).output()?;
    if !output.status.success() {
        anyhow::bail!(
            "{} {} failed: {}",
            llvm_config.display(),
            arg,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn main() -> anyhow::Result<()> {
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=llvm_init_shim.c");

    let llvm_config = find_llvm_config()?;
    println!("cargo:rerun-if-env-changed=LLVM_CONFIG_PATH");

    let include_dir = run_llvm_config(&llvm_config, "--includedir")?;
    let lib_dir = run_llvm_config(&llvm_config, "--libdir")?;
    let libs_raw = run_llvm_config(&llvm_config, "--libs")?;
    let system_libs_raw = run_llvm_config(&llvm_config, "--system-libs")?;

    println!("cargo:rustc-link-search=native={lib_dir}");

    for token in libs_raw
        .split_whitespace()
        .chain(system_libs_raw.split_whitespace())
    {
        if let Some(name) = token.strip_prefix("-l") {
            println!("cargo:rustc-link-lib={name}");
        } else if let Some(path) = token.strip_prefix("-L") {
            println!("cargo:rustc-link-search=native={path}");
        }
    }

    let out_path = PathBuf::from(env::var("OUT_DIR")?);
    let shim_obj = out_path.join("llvm_init_shim.o");
    let shim_lib = out_path.join("libllvm_init_shim.a");

    let compile_status = Command::new("cc")
        .arg("-c")
        .arg("llvm_init_shim.c")
        .arg("-I")
        .arg(&include_dir)
        .arg("-o")
        .arg(&shim_obj)
        .status()?;
    if !compile_status.success() {
        anyhow::bail!("failed to compile llvm_init_shim.c");
    }

    let archive_status = Command::new("ar")
        .arg("crus")
        .arg(&shim_lib)
        .arg(&shim_obj)
        .status()?;
    if !archive_status.success() {
        anyhow::bail!("failed to archive llvm_init_shim.o");
    }

    println!("cargo:rustc-link-search=native={}", out_path.display());
    println!("cargo:rustc-link-lib=static=llvm_init_shim");

    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_arg(format!("-I{include_dir}"))
        .allowlist_function("LLVM.*")
        .allowlist_type("LLVM.*")
        .allowlist_var("LLVM.*")
        .generate_comments(false)
        .generate()
        .map_err(|_| anyhow::anyhow!("failed to generate LLVM bindings"))?;
    bindings.write_to_file(out_path.join("llvm_bindings.rs"))?;

    Ok(())
}
