use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, ValueEnum};
use std::ffi::{CStr, CString};
use std::fs;
use std::os::raw::c_char;
use std::path::{Path, PathBuf};
use std::process::Command;

mod llvm {
    #![allow(non_upper_case_globals)]
    #![allow(dead_code)]
    #![allow(non_camel_case_types)]
    #![allow(non_snake_case)]
    include!(concat!(env!("OUT_DIR"), "/llvm_bindings.rs"));
}

#[derive(Clone, Eq, PartialEq, ValueEnum)]
enum OptLevel {
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
struct Cli {
    /// Input brainfuck source file
    input: PathBuf,

    /// Output native binary path
    #[arg(short, long)]
    output: PathBuf,

    /// Keep temporary object file
    #[arg(long)]
    keep_obj: bool,
    // optimize args
    #[arg(short = 'O')]
    opt: Option<OptLevel>,
}

#[derive(Clone, Copy, Debug)]
enum Op {
    PtrAdd(i64),
    CellAdd(i32),
    Output,
    Input,
    LoopStart,
    LoopEnd,
}

static MODULE_NAME: &str = "bf_module";
static PUTCHAR_NAME: &str = "putchar";
static GETCHAR_NAME: &str = "getchar";
static MAIN_FN_NAME: &str = "main";
static ENTRY_BLOCK_NAME: &str = "entry";
static TAPE_NAME: &str = "tape";
static INDEX_NAME: &str = "idx";
static TARGET_CPU: &str = "generic";
static TAPE_LEN: u64 = 30_000;

fn cstring(s: &str) -> Result<CString> {
    CString::new(s).map_err(|_| anyhow!("string contains interior NUL: {s:?}"))
}

fn parse_brainfuck(source: &str) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    let mut chars = source.chars().peekable();
    let mut stack = Vec::new();

    while let Some(ch) = chars.next() {
        match ch {
            '>' | '<' => {
                let mut delta: i64 = if ch == '>' { 1 } else { -1 };
                while let Some(next) = chars.peek() {
                    if *next == '>' {
                        delta += 1;
                        chars.next();
                    } else if *next == '<' {
                        delta -= 1;
                        chars.next();
                    } else {
                        break;
                    }
                }
                if delta != 0 {
                    ops.push(Op::PtrAdd(delta));
                }
            }
            '+' | '-' => {
                let mut delta: i32 = if ch == '+' { 1 } else { -1 };
                while let Some(next) = chars.peek() {
                    if *next == '+' {
                        delta += 1;
                        chars.next();
                    } else if *next == '-' {
                        delta -= 1;
                        chars.next();
                    } else {
                        break;
                    }
                }
                if delta != 0 {
                    ops.push(Op::CellAdd(delta));
                }
            }
            '.' => ops.push(Op::Output),
            ',' => ops.push(Op::Input),
            '[' => {
                stack.push(ops.len());
                ops.push(Op::LoopStart);
            }
            ']' => {
                if stack.pop().is_none() {
                    bail!("unmatched closing bracket ']' found");
                }
                ops.push(Op::LoopEnd);
            }
            _ => {}
        }
    }

    if !stack.is_empty() {
        bail!("unmatched opening bracket '[' found");
    }

    Ok(ops)
}

unsafe fn llvm_error_to_string(err: *mut c_char) -> String {
    if err.is_null() {
        return "unknown LLVM error".to_string();
    }
    let msg = CStr::from_ptr(err).to_string_lossy().to_string();
    llvm::LLVMDisposeMessage(err);
    msg
}

unsafe fn initialize_llvm_targets() {
    llvm::LLVMInitializeAllTargetInfosShim();
    llvm::LLVMInitializeAllTargetsShim();
    llvm::LLVMInitializeAllTargetMCsShim();
    llvm::LLVMInitializeAllAsmPrintersShim();
    llvm::LLVMInitializeAllAsmParsersShim();
}

struct LLVMCompiler {
    context: llvm::LLVMContextRef,
    module: llvm::LLVMModuleRef,
    builder: llvm::LLVMBuilderRef,
}

impl LLVMCompiler {
    pub unsafe fn new() -> Result<Self> {
        initialize_llvm_targets();

        let context = llvm::LLVMContextCreate();
        if context.is_null() {
            bail!("failed to create LLVM context");
        }

        let module_name = cstring(MODULE_NAME)?;
        let module = llvm::LLVMModuleCreateWithNameInContext(module_name.as_ptr(), context);
        let builder = llvm::LLVMCreateBuilderInContext(context);

        Ok(Self {
            context,
            module,
            builder,
        })
    }
    pub unsafe fn build(&self, ops: &[Op]) -> Result<()> {
        let i8_ty = llvm::LLVMInt8TypeInContext(self.context);
        let i32_ty = llvm::LLVMInt32TypeInContext(self.context);
        let i64_ty = llvm::LLVMInt64TypeInContext(self.context);
        let putchar_ty = llvm::LLVMFunctionType(i32_ty, [i32_ty].as_ptr().cast_mut(), 1, 0);
        let getchar_ty = llvm::LLVMFunctionType(i32_ty, std::ptr::null_mut(), 0, 0);
        let putchar_name = cstring(PUTCHAR_NAME)?;
        let getchar_name = cstring(GETCHAR_NAME)?;
        let putchar_fn = llvm::LLVMAddFunction(self.module, putchar_name.as_ptr(), putchar_ty);
        let getchar_fn = llvm::LLVMAddFunction(self.module, getchar_name.as_ptr(), getchar_ty);

        let main_ty = llvm::LLVMFunctionType(i32_ty, std::ptr::null_mut(), 0, 0);
        let main_name = cstring(MAIN_FN_NAME)?;
        let main_fn = llvm::LLVMAddFunction(self.module, main_name.as_ptr(), main_ty);
        let entry_name = cstring(ENTRY_BLOCK_NAME)?;
        let entry = llvm::LLVMAppendBasicBlockInContext(self.context, main_fn, entry_name.as_ptr());
        llvm::LLVMPositionBuilderAtEnd(self.builder, entry);

        let tape_ty = llvm::LLVMArrayType2(i8_ty, TAPE_LEN);
        let tape_name = cstring(TAPE_NAME)?;
        let tape = llvm::LLVMBuildAlloca(self.builder, tape_ty, tape_name.as_ptr());
        llvm::LLVMBuildMemSet(
            self.builder,
            tape,
            llvm::LLVMConstInt(i8_ty, 0, 0),
            llvm::LLVMConstInt(i64_ty, TAPE_LEN, 0),
            1,
        );

        let idx_name = cstring(INDEX_NAME)?;
        let idx_ptr = llvm::LLVMBuildAlloca(self.builder, i64_ty, idx_name.as_ptr());
        llvm::LLVMBuildStore(self.builder, llvm::LLVMConstInt(i64_ty, 0, 0), idx_ptr);

        let zero_i64 = llvm::LLVMConstInt(i64_ty, 0, 0);

        let mut loop_stack: Vec<(llvm::LLVMBasicBlockRef, llvm::LLVMBasicBlockRef)> = Vec::new();

        for op in ops {
            match *op {
                Op::PtrAdd(delta) => {
                    let cur = llvm::LLVMBuildLoad2(
                        self.builder,
                        i64_ty,
                        idx_ptr,
                        cstring("idx.cur")?.as_ptr(),
                    );
                    let val = llvm::LLVMConstInt(i64_ty, delta.unsigned_abs(), 0);
                    let next = if delta >= 0 {
                        llvm::LLVMBuildAdd(self.builder, cur, val, cstring("idx.add")?.as_ptr())
                    } else {
                        llvm::LLVMBuildSub(self.builder, cur, val, cstring("idx.sub")?.as_ptr())
                    };
                    llvm::LLVMBuildStore(self.builder, next, idx_ptr);
                }
                Op::CellAdd(delta) => {
                    let cur_idx = llvm::LLVMBuildLoad2(
                        self.builder,
                        i64_ty,
                        idx_ptr,
                        cstring("idx.load")?.as_ptr(),
                    );
                    let cell_ptr = llvm::LLVMBuildInBoundsGEP2(
                        self.builder,
                        tape_ty,
                        tape,
                        [zero_i64, cur_idx].as_ptr().cast_mut(),
                        2,
                        cstring("cell.ptr")?.as_ptr(),
                    );
                    let cur_val = llvm::LLVMBuildLoad2(
                        self.builder,
                        i8_ty,
                        cell_ptr,
                        cstring("cell.cur")?.as_ptr(),
                    );
                    let delta_val =
                        llvm::LLVMConstInt(i8_ty, (delta as i64).rem_euclid(256) as u64, 0);
                    let next = llvm::LLVMBuildAdd(
                        self.builder,
                        cur_val,
                        delta_val,
                        cstring("cell.next")?.as_ptr(),
                    );
                    llvm::LLVMBuildStore(self.builder, next, cell_ptr);
                }
                Op::Output => {
                    let cur_idx = llvm::LLVMBuildLoad2(
                        self.builder,
                        i64_ty,
                        idx_ptr,
                        cstring("idx.load")?.as_ptr(),
                    );
                    let cell_ptr = llvm::LLVMBuildInBoundsGEP2(
                        self.builder,
                        tape_ty,
                        tape,
                        [zero_i64, cur_idx].as_ptr().cast_mut(),
                        2,
                        cstring("cell.ptr")?.as_ptr(),
                    );
                    let cell = llvm::LLVMBuildLoad2(
                        self.builder,
                        i8_ty,
                        cell_ptr,
                        cstring("cell.out")?.as_ptr(),
                    );
                    let widened = llvm::LLVMBuildZExt(
                        self.builder,
                        cell,
                        i32_ty,
                        cstring("out.zext")?.as_ptr(),
                    );
                    llvm::LLVMBuildCall2(
                        self.builder,
                        putchar_ty,
                        putchar_fn,
                        [widened].as_ptr().cast_mut(),
                        1,
                        cstring("")?.as_ptr(),
                    );
                }
                Op::Input => {
                    let input = llvm::LLVMBuildCall2(
                        self.builder,
                        getchar_ty,
                        getchar_fn,
                        std::ptr::null_mut(),
                        0,
                        cstring("in")?.as_ptr(),
                    );
                    let byte = llvm::LLVMBuildTrunc(
                        self.builder,
                        input,
                        i8_ty,
                        cstring("in.byte")?.as_ptr(),
                    );
                    let cur_idx = llvm::LLVMBuildLoad2(
                        self.builder,
                        i64_ty,
                        idx_ptr,
                        cstring("idx.load")?.as_ptr(),
                    );
                    let cell_ptr = llvm::LLVMBuildInBoundsGEP2(
                        self.builder,
                        tape_ty,
                        tape,
                        [zero_i64, cur_idx].as_ptr().cast_mut(),
                        2,
                        cstring("cell.ptr")?.as_ptr(),
                    );
                    llvm::LLVMBuildStore(self.builder, byte, cell_ptr);
                }
                Op::LoopStart => {
                    let cond_bb = llvm::LLVMAppendBasicBlockInContext(
                        self.context,
                        main_fn,
                        cstring("loop.cond")?.as_ptr(),
                    );
                    let body_bb = llvm::LLVMAppendBasicBlockInContext(
                        self.context,
                        main_fn,
                        cstring("loop.body")?.as_ptr(),
                    );
                    let end_bb = llvm::LLVMAppendBasicBlockInContext(
                        self.context,
                        main_fn,
                        cstring("loop.end")?.as_ptr(),
                    );

                    llvm::LLVMBuildBr(self.builder, cond_bb);
                    llvm::LLVMPositionBuilderAtEnd(self.builder, cond_bb);

                    let cur_idx = llvm::LLVMBuildLoad2(
                        self.builder,
                        i64_ty,
                        idx_ptr,
                        cstring("idx.loop")?.as_ptr(),
                    );
                    let cell_ptr = llvm::LLVMBuildInBoundsGEP2(
                        self.builder,
                        tape_ty,
                        tape,
                        [zero_i64, cur_idx].as_ptr().cast_mut(),
                        2,
                        cstring("cell.ptr")?.as_ptr(),
                    );
                    let cell = llvm::LLVMBuildLoad2(
                        self.builder,
                        i8_ty,
                        cell_ptr,
                        cstring("cell.loop")?.as_ptr(),
                    );
                    let is_non_zero = llvm::LLVMBuildICmp(
                        self.builder,
                        llvm::LLVMIntPredicate_LLVMIntNE,
                        cell,
                        llvm::LLVMConstInt(i8_ty, 0, 0),
                        cstring("loop.nz")?.as_ptr(),
                    );
                    llvm::LLVMBuildCondBr(self.builder, is_non_zero, body_bb, end_bb);
                    llvm::LLVMPositionBuilderAtEnd(self.builder, body_bb);

                    loop_stack.push((cond_bb, end_bb));
                }
                Op::LoopEnd => {
                    let (cond_bb, end_bb) = loop_stack
                        .pop()
                        .ok_or_else(|| anyhow!("internal loop mismatch"))?;
                    llvm::LLVMBuildBr(self.builder, cond_bb);
                    llvm::LLVMPositionBuilderAtEnd(self.builder, end_bb);
                }
            }
        }

        llvm::LLVMBuildRet(self.builder, llvm::LLVMConstInt(i32_ty, 0, 0));

        if !loop_stack.is_empty() {
            bail!("internal loop stack not empty");
        }

        if llvm::LLVMVerifyModule(
            self.module,
            llvm::LLVMVerifierFailureAction_LLVMReturnStatusAction,
            std::ptr::null_mut(),
        ) != 0
        {
            bail!("LLVM module verification failed");
        }

        Ok(())
    }
}

impl Drop for LLVMCompiler {
    fn drop(&mut self) {
        unsafe {
            llvm::LLVMDisposeBuilder(self.builder);
            llvm::LLVMDisposeModule(self.module);
            llvm::LLVMContextDispose(self.context);
        }
    }
}

struct LLVMTargetMachine {
    tm: llvm::LLVMTargetMachineRef,
}

impl LLVMTargetMachine {
    pub unsafe fn new(
        target: llvm::LLVMTargetRef,
        triple: *mut c_char,
        cpu: *mut c_char,
        features: *mut c_char,
        opt_level: llvm::LLVMCodeGenOptLevel,
    ) -> Result<Self> {
        let tm = llvm::LLVMCreateTargetMachine(
            target,
            triple,
            cpu,
            features,
            opt_level,
            llvm::LLVMRelocMode_LLVMRelocDefault,
            llvm::LLVMCodeModel_LLVMCodeModelDefault,
        );
        if tm.is_null() {
            bail!("failed to create target machine");
        }
        Ok(Self { tm })
    }
}

impl Drop for LLVMTargetMachine {
    fn drop(&mut self) {
        unsafe {
            llvm::LLVMDisposeTargetMachine(self.tm);
        }
    }
}

struct LLVMTriple {
    triple: *mut c_char,
}

impl LLVMTriple {
    pub unsafe fn new() -> Self {
        Self {
            triple: llvm::LLVMGetDefaultTargetTriple(),
        }
    }
}

impl Drop for LLVMTriple {
    fn drop(&mut self) {
        unsafe {
            llvm::LLVMDisposeMessage(self.triple);
        }
    }
}

unsafe fn compile_to_object(ops: &[Op], object_path: &Path, opt_level: &OptLevel) -> Result<()> {
    let compiler = LLVMCompiler::new()?;
    compiler.build(ops)?;
    let triple = LLVMTriple::new();

    let mut target = std::ptr::null_mut();
    let mut target_err = std::ptr::null_mut();
    if llvm::LLVMGetTargetFromTriple(triple.triple, &mut target, &mut target_err) != 0 {
        let msg = llvm_error_to_string(target_err);
        llvm::LLVMDisposeMessage(triple.triple);
        bail!("failed to get target from triple: {msg}");
    }
    let level = match opt_level {
        OptLevel::O0 => llvm::LLVMCodeGenOptLevel_LLVMCodeGenLevelNone,
        OptLevel::O1 => llvm::LLVMCodeGenOptLevel_LLVMCodeGenLevelLess,
        OptLevel::O2 => llvm::LLVMCodeGenOptLevel_LLVMCodeGenLevelDefault,
        OptLevel::O3 => llvm::LLVMCodeGenOptLevel_LLVMCodeGenLevelAggressive,
    };
    let tm = LLVMTargetMachine::new(
        target,
        triple.triple,
        cstring(TARGET_CPU)?.as_ptr() as *mut _,
        cstring("")?.as_ptr() as *mut _,
        level,
    )?;

    llvm::LLVMSetTarget(compiler.module, triple.triple);

    let data_layout = llvm::LLVMCreateTargetDataLayout(tm.tm);
    let layout_str = llvm::LLVMCopyStringRepOfTargetData(data_layout);
    llvm::LLVMSetDataLayout(compiler.module, layout_str);
    llvm::LLVMDisposeMessage(layout_str);
    llvm::LLVMDisposeTargetData(data_layout);

    let mut emit_err = std::ptr::null_mut();
    let object_c = cstring(&object_path.to_string_lossy())?;
    if llvm::LLVMTargetMachineEmitToFile(
        tm.tm,
        compiler.module,
        object_c.as_ptr().cast_mut(),
        llvm::LLVMCodeGenFileType_LLVMObjectFile,
        &mut emit_err,
    ) != 0
    {
        let msg = llvm_error_to_string(emit_err);
        bail!("failed to emit object file: {msg}");
    }

    Ok(())
}

fn link_executable(object_path: &Path, output_path: &Path, opt_level: &OptLevel) -> Result<()> {
    let status = Command::new("cc")
        .arg(object_path)
        .arg("-o")
        .arg(output_path)
        .arg(match opt_level {
            OptLevel::O0 => "-O0",
            OptLevel::O1 => "-O1",
            OptLevel::O2 => "-O2",
            OptLevel::O3 => "-O3",
        })
        .status()
        .context("failed to invoke system C compiler (cc)")?;

    if !status.success() {
        bail!("linker failed with status {status}");
    }

    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let source = fs::read_to_string(&cli.input)
        .with_context(|| format!("failed to read input file: {}", cli.input.display()))?;
    let ops = parse_brainfuck(&source)?;

    let object_path = cli.output.with_extension("o");
    let opt = cli.opt.unwrap_or(OptLevel::O0);
    unsafe { compile_to_object(&ops, &object_path, &opt)? };
    link_executable(&object_path, &cli.output, &opt)?;

    if !cli.keep_obj {
        let _ = fs::remove_file(&object_path);
    }

    println!("generated executable: {}", cli.output.display());

    Ok(())
}
