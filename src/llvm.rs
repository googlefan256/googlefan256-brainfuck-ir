use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, ValueEnum};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::parser::Op;

mod llvm_native {
    #![allow(non_upper_case_globals)]
    #![allow(dead_code)]
    #![allow(non_camel_case_types)]
    #![allow(non_snake_case)]
    include!(concat!(env!("OUT_DIR"), "/llvm_bindings.rs"));
}

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

impl OptLevel {
    fn llvm_codegen_level(self) -> llvm_native::LLVMCodeGenOptLevel {
        match self {
            Self::O0 => llvm_native::LLVMCodeGenOptLevel_LLVMCodeGenLevelNone,
            Self::O1 => llvm_native::LLVMCodeGenOptLevel_LLVMCodeGenLevelLess,
            Self::O2 => llvm_native::LLVMCodeGenOptLevel_LLVMCodeGenLevelDefault,
            Self::O3 => llvm_native::LLVMCodeGenOptLevel_LLVMCodeGenLevelAggressive,
        }
    }

    fn llvm_pass_pipeline(self) -> &'static str {
        match self {
            Self::O0 => "default<O0>",
            Self::O1 => "default<O1>",
            Self::O2 => "default<O2>",
            Self::O3 => "default<O3>",
        }
    }

    fn cc_opt_flag(self) -> &'static str {
        match self {
            Self::O0 => "-O0",
            Self::O1 => "-O1",
            Self::O2 => "-O2",
            Self::O3 => "-O3",
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
    pub output: PathBuf,

    /// Keep temporary object file
    #[arg(long)]
    pub keep_obj: bool,
    // optimize args
    #[arg(short = 'O')]
    pub opt: Option<OptLevel>,
    /// Run binary
    #[arg(long)]
    pub run: bool,
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

unsafe fn llvm_error_to_string(err: *mut c_char) -> String {
    if err.is_null() {
        return "unknown LLVM error".to_string();
    }
    let msg = CStr::from_ptr(err).to_string_lossy().to_string();
    llvm_native::LLVMDisposeMessage(err);
    msg
}

unsafe fn llvm_error_ref_to_string(err: llvm_native::LLVMErrorRef) -> String {
    if err.is_null() {
        return "unknown LLVM error".to_string();
    }
    let msg = llvm_native::LLVMGetErrorMessage(err);
    let text = CStr::from_ptr(msg).to_string_lossy().to_string();
    llvm_native::LLVMDisposeErrorMessage(msg);
    text
}

unsafe fn initialize_llvm_targets() {
    llvm_native::LLVMInitializeAllTargetInfosShim();
    llvm_native::LLVMInitializeAllTargetsShim();
    llvm_native::LLVMInitializeAllTargetMCsShim();
    llvm_native::LLVMInitializeAllAsmPrintersShim();
    llvm_native::LLVMInitializeAllAsmParsersShim();
}

struct LLVMCompiler {
    context: llvm_native::LLVMContextRef,
    module: llvm_native::LLVMModuleRef,
    builder: llvm_native::LLVMBuilderRef,
}

impl LLVMCompiler {
    pub unsafe fn new() -> Result<Self> {
        initialize_llvm_targets();

        let context = llvm_native::LLVMContextCreate();
        if context.is_null() {
            bail!("failed to create LLVM context");
        }

        let module_name = cstring(MODULE_NAME)?;
        let module = llvm_native::LLVMModuleCreateWithNameInContext(module_name.as_ptr(), context);
        let builder = llvm_native::LLVMCreateBuilderInContext(context);

        Ok(Self {
            context,
            module,
            builder,
        })
    }

    unsafe fn build_cell_ptr(
        &self,
        tape_ty: llvm_native::LLVMTypeRef,
        tape: llvm_native::LLVMValueRef,
        idx: llvm_native::LLVMValueRef,
        zero_i64: llvm_native::LLVMValueRef,
        name: &str,
    ) -> Result<llvm_native::LLVMValueRef> {
        Ok(llvm_native::LLVMBuildInBoundsGEP2(
            self.builder,
            tape_ty,
            tape,
            [zero_i64, idx].as_ptr().cast_mut(),
            2,
            cstring(name)?.as_ptr(),
        ))
    }
    #[allow(clippy::too_many_arguments)]
    unsafe fn build_current_cell_ptr(
        &self,
        tape_ty: llvm_native::LLVMTypeRef,
        tape: llvm_native::LLVMValueRef,
        idx_ptr: llvm_native::LLVMValueRef,
        i64_ty: llvm_native::LLVMTypeRef,
        zero_i64: llvm_native::LLVMValueRef,
        idx_name: &str,
        ptr_name: &str,
    ) -> Result<(llvm_native::LLVMValueRef, llvm_native::LLVMValueRef)> {
        let cur_idx =
            llvm_native::LLVMBuildLoad2(self.builder, i64_ty, idx_ptr, cstring(idx_name)?.as_ptr());
        let cell_ptr = self.build_cell_ptr(tape_ty, tape, cur_idx, zero_i64, ptr_name)?;
        Ok((cur_idx, cell_ptr))
    }

    pub unsafe fn build(&self, ops: &[Op]) -> Result<()> {
        let i8_ty = llvm_native::LLVMInt8TypeInContext(self.context);
        let i32_ty = llvm_native::LLVMInt32TypeInContext(self.context);
        let i64_ty = llvm_native::LLVMInt64TypeInContext(self.context);
        let putchar_ty = llvm_native::LLVMFunctionType(i32_ty, [i32_ty].as_ptr().cast_mut(), 1, 0);
        let getchar_ty = llvm_native::LLVMFunctionType(i32_ty, std::ptr::null_mut(), 0, 0);
        let putchar_name = cstring(PUTCHAR_NAME)?;
        let getchar_name = cstring(GETCHAR_NAME)?;
        let putchar_fn =
            llvm_native::LLVMAddFunction(self.module, putchar_name.as_ptr(), putchar_ty);
        let getchar_fn =
            llvm_native::LLVMAddFunction(self.module, getchar_name.as_ptr(), getchar_ty);

        let main_ty = llvm_native::LLVMFunctionType(i32_ty, std::ptr::null_mut(), 0, 0);
        let main_name = cstring(MAIN_FN_NAME)?;
        let main_fn = llvm_native::LLVMAddFunction(self.module, main_name.as_ptr(), main_ty);
        let entry_name = cstring(ENTRY_BLOCK_NAME)?;
        let entry =
            llvm_native::LLVMAppendBasicBlockInContext(self.context, main_fn, entry_name.as_ptr());
        llvm_native::LLVMPositionBuilderAtEnd(self.builder, entry);

        let tape_ty = llvm_native::LLVMArrayType2(i8_ty, TAPE_LEN);
        let tape_name = cstring(TAPE_NAME)?;
        let tape = llvm_native::LLVMBuildAlloca(self.builder, tape_ty, tape_name.as_ptr());
        llvm_native::LLVMBuildMemSet(
            self.builder,
            tape,
            llvm_native::LLVMConstInt(i8_ty, 0, 0),
            llvm_native::LLVMConstInt(i64_ty, TAPE_LEN, 0),
            1,
        );

        let idx_name = cstring(INDEX_NAME)?;
        let idx_ptr = llvm_native::LLVMBuildAlloca(self.builder, i64_ty, idx_name.as_ptr());
        llvm_native::LLVMBuildStore(
            self.builder,
            llvm_native::LLVMConstInt(i64_ty, 0, 0),
            idx_ptr,
        );

        let zero_i64 = llvm_native::LLVMConstInt(i64_ty, 0, 0);

        let mut loop_stack: Vec<(
            llvm_native::LLVMBasicBlockRef,
            llvm_native::LLVMBasicBlockRef,
        )> = Vec::new();

        for op in ops {
            match op {
                Op::PtrAdd(delta) => {
                    let cur = llvm_native::LLVMBuildLoad2(
                        self.builder,
                        i64_ty,
                        idx_ptr,
                        cstring("idx.cur")?.as_ptr(),
                    );
                    let val = llvm_native::LLVMConstInt(i64_ty, delta.unsigned_abs(), 0);
                    let next = if *delta >= 0 {
                        llvm_native::LLVMBuildAdd(
                            self.builder,
                            cur,
                            val,
                            cstring("idx.add")?.as_ptr(),
                        )
                    } else {
                        llvm_native::LLVMBuildSub(
                            self.builder,
                            cur,
                            val,
                            cstring("idx.sub")?.as_ptr(),
                        )
                    };
                    llvm_native::LLVMBuildStore(self.builder, next, idx_ptr);
                }
                Op::CellAdd(delta) => {
                    let (_, cell_ptr) = self.build_current_cell_ptr(
                        tape_ty, tape, idx_ptr, i64_ty, zero_i64, "idx.load", "cell.ptr",
                    )?;
                    let cur_val = llvm_native::LLVMBuildLoad2(
                        self.builder,
                        i8_ty,
                        cell_ptr,
                        cstring("cell.cur")?.as_ptr(),
                    );
                    let delta_val =
                        llvm_native::LLVMConstInt(i8_ty, (*delta as i64).rem_euclid(256) as u64, 0);
                    let next = llvm_native::LLVMBuildAdd(
                        self.builder,
                        cur_val,
                        delta_val,
                        cstring("cell.next")?.as_ptr(),
                    );
                    llvm_native::LLVMBuildStore(self.builder, next, cell_ptr);
                }
                Op::ClearCell => {
                    let (_, cell_ptr) = self.build_current_cell_ptr(
                        tape_ty, tape, idx_ptr, i64_ty, zero_i64, "idx.load", "cell.ptr",
                    )?;
                    llvm_native::LLVMBuildStore(
                        self.builder,
                        llvm_native::LLVMConstInt(i8_ty, 0, 0),
                        cell_ptr,
                    );
                }
                Op::AddScaled(updates) => {
                    let (cur_idx, cell_ptr) = self.build_current_cell_ptr(
                        tape_ty, tape, idx_ptr, i64_ty, zero_i64, "idx.load", "cell.ptr",
                    )?;
                    let cell = llvm_native::LLVMBuildLoad2(
                        self.builder,
                        i8_ty,
                        cell_ptr,
                        cstring("cell.mul")?.as_ptr(),
                    );

                    for update in updates {
                        let offset =
                            llvm_native::LLVMConstInt(i64_ty, update.offset.unsigned_abs(), 0);
                        let target_idx = if update.offset >= 0 {
                            llvm_native::LLVMBuildAdd(
                                self.builder,
                                cur_idx,
                                offset,
                                cstring("idx.scaled.add")?.as_ptr(),
                            )
                        } else {
                            llvm_native::LLVMBuildSub(
                                self.builder,
                                cur_idx,
                                offset,
                                cstring("idx.scaled.sub")?.as_ptr(),
                            )
                        };
                        let target_ptr = self.build_cell_ptr(
                            tape_ty,
                            tape,
                            target_idx,
                            zero_i64,
                            "cell.scaled.ptr",
                        )?;
                        let target = llvm_native::LLVMBuildLoad2(
                            self.builder,
                            i8_ty,
                            target_ptr,
                            cstring("cell.scaled.cur")?.as_ptr(),
                        );
                        let scale = llvm_native::LLVMConstInt(
                            i8_ty,
                            (update.factor as i64).rem_euclid(256) as u64,
                            0,
                        );
                        let scaled = llvm_native::LLVMBuildMul(
                            self.builder,
                            cell,
                            scale,
                            cstring("cell.scaled.mul")?.as_ptr(),
                        );
                        let next = llvm_native::LLVMBuildAdd(
                            self.builder,
                            target,
                            scaled,
                            cstring("cell.scaled.next")?.as_ptr(),
                        );
                        llvm_native::LLVMBuildStore(self.builder, next, target_ptr);
                    }

                    llvm_native::LLVMBuildStore(
                        self.builder,
                        llvm_native::LLVMConstInt(i8_ty, 0, 0),
                        cell_ptr,
                    );
                }
                Op::Output => {
                    let (_, cell_ptr) = self.build_current_cell_ptr(
                        tape_ty, tape, idx_ptr, i64_ty, zero_i64, "idx.load", "cell.ptr",
                    )?;
                    let cell = llvm_native::LLVMBuildLoad2(
                        self.builder,
                        i8_ty,
                        cell_ptr,
                        cstring("cell.out")?.as_ptr(),
                    );
                    let widened = llvm_native::LLVMBuildZExt(
                        self.builder,
                        cell,
                        i32_ty,
                        cstring("out.zext")?.as_ptr(),
                    );
                    llvm_native::LLVMBuildCall2(
                        self.builder,
                        putchar_ty,
                        putchar_fn,
                        [widened].as_ptr().cast_mut(),
                        1,
                        cstring("")?.as_ptr(),
                    );
                }
                Op::Input => {
                    let input = llvm_native::LLVMBuildCall2(
                        self.builder,
                        getchar_ty,
                        getchar_fn,
                        std::ptr::null_mut(),
                        0,
                        cstring("in")?.as_ptr(),
                    );
                    let byte = llvm_native::LLVMBuildTrunc(
                        self.builder,
                        input,
                        i8_ty,
                        cstring("in.byte")?.as_ptr(),
                    );
                    let (_, cell_ptr) = self.build_current_cell_ptr(
                        tape_ty, tape, idx_ptr, i64_ty, zero_i64, "idx.load", "cell.ptr",
                    )?;
                    llvm_native::LLVMBuildStore(self.builder, byte, cell_ptr);
                }
                Op::LoopStart => {
                    let cond_bb = llvm_native::LLVMAppendBasicBlockInContext(
                        self.context,
                        main_fn,
                        cstring("loop.cond")?.as_ptr(),
                    );
                    let body_bb = llvm_native::LLVMAppendBasicBlockInContext(
                        self.context,
                        main_fn,
                        cstring("loop.body")?.as_ptr(),
                    );
                    let end_bb = llvm_native::LLVMAppendBasicBlockInContext(
                        self.context,
                        main_fn,
                        cstring("loop.end")?.as_ptr(),
                    );

                    llvm_native::LLVMBuildBr(self.builder, cond_bb);
                    llvm_native::LLVMPositionBuilderAtEnd(self.builder, cond_bb);

                    let (_, cell_ptr) = self.build_current_cell_ptr(
                        tape_ty, tape, idx_ptr, i64_ty, zero_i64, "idx.loop", "cell.ptr",
                    )?;
                    let cell = llvm_native::LLVMBuildLoad2(
                        self.builder,
                        i8_ty,
                        cell_ptr,
                        cstring("cell.loop")?.as_ptr(),
                    );
                    let is_non_zero = llvm_native::LLVMBuildICmp(
                        self.builder,
                        llvm_native::LLVMIntPredicate_LLVMIntNE,
                        cell,
                        llvm_native::LLVMConstInt(i8_ty, 0, 0),
                        cstring("loop.nz")?.as_ptr(),
                    );
                    llvm_native::LLVMBuildCondBr(self.builder, is_non_zero, body_bb, end_bb);
                    llvm_native::LLVMPositionBuilderAtEnd(self.builder, body_bb);

                    loop_stack.push((cond_bb, end_bb));
                }
                Op::LoopEnd => {
                    let (cond_bb, end_bb) = loop_stack
                        .pop()
                        .ok_or_else(|| anyhow!("internal loop mismatch"))?;
                    llvm_native::LLVMBuildBr(self.builder, cond_bb);
                    llvm_native::LLVMPositionBuilderAtEnd(self.builder, end_bb);
                }
            }
        }

        llvm_native::LLVMBuildRet(self.builder, llvm_native::LLVMConstInt(i32_ty, 0, 0));

        if !loop_stack.is_empty() {
            bail!("internal loop stack not empty");
        }

        if llvm_native::LLVMVerifyModule(
            self.module,
            llvm_native::LLVMVerifierFailureAction_LLVMReturnStatusAction,
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
            llvm_native::LLVMDisposeBuilder(self.builder);
            llvm_native::LLVMDisposeModule(self.module);
            llvm_native::LLVMContextDispose(self.context);
        }
    }
}

struct LLVMTargetMachine {
    tm: llvm_native::LLVMTargetMachineRef,
}

impl LLVMTargetMachine {
    pub unsafe fn new(
        target: llvm_native::LLVMTargetRef,
        triple: *mut c_char,
        cpu: *mut c_char,
        features: *mut c_char,
        opt_level: llvm_native::LLVMCodeGenOptLevel,
    ) -> Result<Self> {
        let tm = llvm_native::LLVMCreateTargetMachine(
            target,
            triple,
            cpu,
            features,
            opt_level,
            llvm_native::LLVMRelocMode_LLVMRelocDefault,
            llvm_native::LLVMCodeModel_LLVMCodeModelDefault,
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
            llvm_native::LLVMDisposeTargetMachine(self.tm);
        }
    }
}

struct LLVMTriple {
    triple: *mut c_char,
}

impl LLVMTriple {
    pub unsafe fn new() -> Self {
        Self {
            triple: llvm_native::LLVMGetDefaultTargetTriple(),
        }
    }
}

impl Drop for LLVMTriple {
    fn drop(&mut self) {
        unsafe {
            llvm_native::LLVMDisposeMessage(self.triple);
        }
    }
}

unsafe fn run_llvm_passes(
    module: llvm_native::LLVMModuleRef,
    target_machine: llvm_native::LLVMTargetMachineRef,
    opt_level: OptLevel,
) -> Result<()> {
    let pass_pipeline = cstring(opt_level.llvm_pass_pipeline())?;
    let options = llvm_native::LLVMCreatePassBuilderOptions();
    if options.is_null() {
        bail!("failed to create LLVM pass builder options");
    }

    let err = llvm_native::LLVMRunPasses(module, pass_pipeline.as_ptr(), target_machine, options);
    llvm_native::LLVMDisposePassBuilderOptions(options);

    if !err.is_null() {
        bail!(
            "failed to run LLVM optimization passes: {}",
            llvm_error_ref_to_string(err)
        );
    }

    Ok(())
}

pub unsafe fn compile_to_object(
    ops: &[Op],
    object_path: &Path,
    opt_level: &OptLevel,
) -> Result<()> {
    let compiler = LLVMCompiler::new()?;
    compiler.build(ops)?;
    let triple = LLVMTriple::new();

    let mut target = std::ptr::null_mut();
    let mut target_err = std::ptr::null_mut();
    if llvm_native::LLVMGetTargetFromTriple(triple.triple, &mut target, &mut target_err) != 0 {
        let msg = llvm_error_to_string(target_err);
        bail!("failed to get target from triple: {msg}");
    }
    let tm = LLVMTargetMachine::new(
        target,
        triple.triple,
        cstring(TARGET_CPU)?.as_ptr() as *mut _,
        cstring("")?.as_ptr() as *mut _,
        opt_level.llvm_codegen_level(),
    )?;

    llvm_native::LLVMSetTarget(compiler.module, triple.triple);

    let data_layout = llvm_native::LLVMCreateTargetDataLayout(tm.tm);
    let layout_str = llvm_native::LLVMCopyStringRepOfTargetData(data_layout);
    llvm_native::LLVMSetDataLayout(compiler.module, layout_str);
    llvm_native::LLVMDisposeMessage(layout_str);
    llvm_native::LLVMDisposeTargetData(data_layout);

    run_llvm_passes(compiler.module, tm.tm, *opt_level)?;

    let mut emit_err = std::ptr::null_mut();
    let object_c = cstring(&object_path.to_string_lossy())?;
    if llvm_native::LLVMTargetMachineEmitToFile(
        tm.tm,
        compiler.module,
        object_c.as_ptr().cast_mut(),
        llvm_native::LLVMCodeGenFileType_LLVMObjectFile,
        &mut emit_err,
    ) != 0
    {
        let msg = llvm_error_to_string(emit_err);
        bail!("failed to emit object file: {msg}");
    }

    Ok(())
}

pub fn link_executable(
    object_path: &Path,
    output_path: &Path,
    opt_level: &OptLevel,
) -> Result<PathBuf> {
    let status = Command::new("cc")
        .arg(object_path)
        .arg("-o")
        .arg(output_path)
        .arg(opt_level.cc_opt_flag())
        .status()
        .context("failed to invoke system C compiler (cc)")?;

    if !status.success() {
        bail!("linker failed with status {status}");
    }

    Ok(output_path.to_path_buf())
}
