use std::path::Path;
use std::process::Command;

use anyhow::Context;

use crate::args::OptLevel;
use crate::llvm::ffi::cstring;
use crate::llvm::memory_buffer::LLVMMemoryBuffer;
use crate::llvm::pass_builder_options::PassBuilderOptions;
use crate::llvm::r#trait::{AsRaw, AsRawMut};
use crate::llvm::target::LLVMTarget;
use crate::llvm::target_machine::LLVMTargetMachine;
use crate::llvm::triple::LLVMTargetTriple;
use crate::parser::Op;
use ffi::{with_llvm_error, with_llvm_error_ref};
mod ffi;
mod memory_buffer;
mod pass_builder_options;
mod target;
mod target_machine;
mod r#trait;
mod triple;

impl OptLevel {
    fn llvm_codegen_level(self) -> ffi::LLVMCodeGenOptLevel {
        match self {
            Self::O0 => ffi::LLVMCodeGenOptLevel_LLVMCodeGenLevelNone,
            Self::O1 => ffi::LLVMCodeGenOptLevel_LLVMCodeGenLevelLess,
            Self::O2 => ffi::LLVMCodeGenOptLevel_LLVMCodeGenLevelDefault,
            Self::O3 => ffi::LLVMCodeGenOptLevel_LLVMCodeGenLevelAggressive,
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

static MODULE_NAME: &str = "bf_module";
static PUTCHAR_NAME: &str = "putchar";
static GETCHAR_NAME: &str = "getchar";
static MAIN_FN_NAME: &str = "main";
static ENTRY_BLOCK_NAME: &str = "entry";
static TAPE_NAME: &str = "tape";
static INDEX_NAME: &str = "idx";
static TARGET_CPU: &str = "generic";
static TAPE_LEN: u64 = 30_000;

unsafe fn initialize_llvm_targets() {
    ffi::LLVMInitializeAllTargetInfosShim();
    ffi::LLVMInitializeAllTargetsShim();
    ffi::LLVMInitializeAllTargetMCsShim();
    ffi::LLVMInitializeAllAsmPrintersShim();
    ffi::LLVMInitializeAllAsmParsersShim();
}

struct LLVMCompiler {
    context: ffi::LLVMContextRef,
    module: ffi::LLVMModuleRef,
    builder: ffi::LLVMBuilderRef,
}

impl LLVMCompiler {
    pub fn new() -> anyhow::Result<Self> {
        unsafe { initialize_llvm_targets() };

        let context = unsafe { ffi::LLVMContextCreate() };
        if context.is_null() {
            anyhow::bail!("failed to create LLVM context");
        }

        let module_name = cstring(MODULE_NAME)?;
        let module =
            unsafe { ffi::LLVMModuleCreateWithNameInContext(module_name.as_ptr(), context) };
        let builder = unsafe { ffi::LLVMCreateBuilderInContext(context) };

        Ok(Self {
            context,
            module,
            builder,
        })
    }

    unsafe fn build_cell_ptr(
        &self,
        tape_ty: ffi::LLVMTypeRef,
        tape: ffi::LLVMValueRef,
        idx: ffi::LLVMValueRef,
        zero_i64: ffi::LLVMValueRef,
        name: &str,
    ) -> anyhow::Result<ffi::LLVMValueRef> {
        Ok(ffi::LLVMBuildInBoundsGEP2(
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
        tape_ty: ffi::LLVMTypeRef,
        tape: ffi::LLVMValueRef,
        idx_ptr: ffi::LLVMValueRef,
        i64_ty: ffi::LLVMTypeRef,
        zero_i64: ffi::LLVMValueRef,
        idx_name: &str,
        ptr_name: &str,
    ) -> anyhow::Result<(ffi::LLVMValueRef, ffi::LLVMValueRef)> {
        let cur_idx =
            ffi::LLVMBuildLoad2(self.builder, i64_ty, idx_ptr, cstring(idx_name)?.as_ptr());
        let cell_ptr = self.build_cell_ptr(tape_ty, tape, cur_idx, zero_i64, ptr_name)?;
        Ok((cur_idx, cell_ptr))
    }

    pub unsafe fn build(&self, ops: &[Op]) -> anyhow::Result<()> {
        let i8_ty = ffi::LLVMInt8TypeInContext(self.context);
        let i32_ty = ffi::LLVMInt32TypeInContext(self.context);
        let i64_ty = ffi::LLVMInt64TypeInContext(self.context);
        let putchar_ty = ffi::LLVMFunctionType(i32_ty, [i32_ty].as_ptr().cast_mut(), 1, 0);
        let getchar_ty = ffi::LLVMFunctionType(i32_ty, std::ptr::null_mut(), 0, 0);
        let putchar_name = cstring(PUTCHAR_NAME)?;
        let getchar_name = cstring(GETCHAR_NAME)?;
        let putchar_fn = ffi::LLVMAddFunction(self.module, putchar_name.as_ptr(), putchar_ty);
        let getchar_fn = ffi::LLVMAddFunction(self.module, getchar_name.as_ptr(), getchar_ty);

        let main_ty = ffi::LLVMFunctionType(i32_ty, std::ptr::null_mut(), 0, 0);
        let main_name = cstring(MAIN_FN_NAME)?;
        let main_fn = ffi::LLVMAddFunction(self.module, main_name.as_ptr(), main_ty);
        let entry_name = cstring(ENTRY_BLOCK_NAME)?;
        let entry = ffi::LLVMAppendBasicBlockInContext(self.context, main_fn, entry_name.as_ptr());
        ffi::LLVMPositionBuilderAtEnd(self.builder, entry);

        let tape_ty = ffi::LLVMArrayType2(i8_ty, TAPE_LEN);
        let tape_name = cstring(TAPE_NAME)?;
        let tape = ffi::LLVMBuildAlloca(self.builder, tape_ty, tape_name.as_ptr());
        ffi::LLVMBuildMemSet(
            self.builder,
            tape,
            ffi::LLVMConstInt(i8_ty, 0, 0),
            ffi::LLVMConstInt(i64_ty, TAPE_LEN, 0),
            1,
        );

        let idx_name = cstring(INDEX_NAME)?;
        let idx_ptr = ffi::LLVMBuildAlloca(self.builder, i64_ty, idx_name.as_ptr());
        ffi::LLVMBuildStore(self.builder, ffi::LLVMConstInt(i64_ty, 0, 0), idx_ptr);

        let zero_i64 = ffi::LLVMConstInt(i64_ty, 0, 0);

        let mut loop_stack: Vec<(ffi::LLVMBasicBlockRef, ffi::LLVMBasicBlockRef)> = Vec::new();

        for op in ops {
            match op {
                Op::PtrAdd(delta) => {
                    let cur = ffi::LLVMBuildLoad2(
                        self.builder,
                        i64_ty,
                        idx_ptr,
                        cstring("idx.cur")?.as_ptr(),
                    );
                    let val = ffi::LLVMConstInt(i64_ty, delta.unsigned_abs(), 0);
                    let next = if *delta >= 0 {
                        ffi::LLVMBuildAdd(self.builder, cur, val, cstring("idx.add")?.as_ptr())
                    } else {
                        ffi::LLVMBuildSub(self.builder, cur, val, cstring("idx.sub")?.as_ptr())
                    };
                    ffi::LLVMBuildStore(self.builder, next, idx_ptr);
                }
                Op::CellAdd(delta) => {
                    let (_, cell_ptr) = self.build_current_cell_ptr(
                        tape_ty, tape, idx_ptr, i64_ty, zero_i64, "idx.load", "cell.ptr",
                    )?;
                    let cur_val = ffi::LLVMBuildLoad2(
                        self.builder,
                        i8_ty,
                        cell_ptr,
                        cstring("cell.cur")?.as_ptr(),
                    );
                    let delta_val =
                        ffi::LLVMConstInt(i8_ty, (*delta as i64).rem_euclid(256) as u64, 0);
                    let next = ffi::LLVMBuildAdd(
                        self.builder,
                        cur_val,
                        delta_val,
                        cstring("cell.next")?.as_ptr(),
                    );
                    ffi::LLVMBuildStore(self.builder, next, cell_ptr);
                }
                Op::ClearCell => {
                    let (_, cell_ptr) = self.build_current_cell_ptr(
                        tape_ty, tape, idx_ptr, i64_ty, zero_i64, "idx.load", "cell.ptr",
                    )?;
                    ffi::LLVMBuildStore(self.builder, ffi::LLVMConstInt(i8_ty, 0, 0), cell_ptr);
                }
                Op::AddScaled(updates) => {
                    let (cur_idx, cell_ptr) = self.build_current_cell_ptr(
                        tape_ty, tape, idx_ptr, i64_ty, zero_i64, "idx.load", "cell.ptr",
                    )?;
                    let cell = ffi::LLVMBuildLoad2(
                        self.builder,
                        i8_ty,
                        cell_ptr,
                        cstring("cell.mul")?.as_ptr(),
                    );

                    for update in updates {
                        let offset = ffi::LLVMConstInt(i64_ty, update.offset.unsigned_abs(), 0);
                        let target_idx = if update.offset >= 0 {
                            ffi::LLVMBuildAdd(
                                self.builder,
                                cur_idx,
                                offset,
                                cstring("idx.scaled.add")?.as_ptr(),
                            )
                        } else {
                            ffi::LLVMBuildSub(
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
                        let target = ffi::LLVMBuildLoad2(
                            self.builder,
                            i8_ty,
                            target_ptr,
                            cstring("cell.scaled.cur")?.as_ptr(),
                        );
                        let scale = ffi::LLVMConstInt(
                            i8_ty,
                            (update.factor as i64).rem_euclid(256) as u64,
                            0,
                        );
                        let scaled = ffi::LLVMBuildMul(
                            self.builder,
                            cell,
                            scale,
                            cstring("cell.scaled.mul")?.as_ptr(),
                        );
                        let next = ffi::LLVMBuildAdd(
                            self.builder,
                            target,
                            scaled,
                            cstring("cell.scaled.next")?.as_ptr(),
                        );
                        ffi::LLVMBuildStore(self.builder, next, target_ptr);
                    }

                    ffi::LLVMBuildStore(self.builder, ffi::LLVMConstInt(i8_ty, 0, 0), cell_ptr);
                }
                Op::Output => {
                    let (_, cell_ptr) = self.build_current_cell_ptr(
                        tape_ty, tape, idx_ptr, i64_ty, zero_i64, "idx.load", "cell.ptr",
                    )?;
                    let cell = ffi::LLVMBuildLoad2(
                        self.builder,
                        i8_ty,
                        cell_ptr,
                        cstring("cell.out")?.as_ptr(),
                    );
                    let widened = ffi::LLVMBuildZExt(
                        self.builder,
                        cell,
                        i32_ty,
                        cstring("out.zext")?.as_ptr(),
                    );
                    ffi::LLVMBuildCall2(
                        self.builder,
                        putchar_ty,
                        putchar_fn,
                        [widened].as_ptr().cast_mut(),
                        1,
                        cstring("")?.as_ptr(),
                    );
                }
                Op::Input => {
                    let input = ffi::LLVMBuildCall2(
                        self.builder,
                        getchar_ty,
                        getchar_fn,
                        std::ptr::null_mut(),
                        0,
                        cstring("in")?.as_ptr(),
                    );
                    let byte = ffi::LLVMBuildTrunc(
                        self.builder,
                        input,
                        i8_ty,
                        cstring("in.byte")?.as_ptr(),
                    );
                    let (_, cell_ptr) = self.build_current_cell_ptr(
                        tape_ty, tape, idx_ptr, i64_ty, zero_i64, "idx.load", "cell.ptr",
                    )?;
                    ffi::LLVMBuildStore(self.builder, byte, cell_ptr);
                }
                Op::LoopStart => {
                    let cond_bb = ffi::LLVMAppendBasicBlockInContext(
                        self.context,
                        main_fn,
                        cstring("loop.cond")?.as_ptr(),
                    );
                    let body_bb = ffi::LLVMAppendBasicBlockInContext(
                        self.context,
                        main_fn,
                        cstring("loop.body")?.as_ptr(),
                    );
                    let end_bb = ffi::LLVMAppendBasicBlockInContext(
                        self.context,
                        main_fn,
                        cstring("loop.end")?.as_ptr(),
                    );

                    ffi::LLVMBuildBr(self.builder, cond_bb);
                    ffi::LLVMPositionBuilderAtEnd(self.builder, cond_bb);

                    let (_, cell_ptr) = self.build_current_cell_ptr(
                        tape_ty, tape, idx_ptr, i64_ty, zero_i64, "idx.loop", "cell.ptr",
                    )?;
                    let cell = ffi::LLVMBuildLoad2(
                        self.builder,
                        i8_ty,
                        cell_ptr,
                        cstring("cell.loop")?.as_ptr(),
                    );
                    let is_non_zero = ffi::LLVMBuildICmp(
                        self.builder,
                        ffi::LLVMIntPredicate_LLVMIntNE,
                        cell,
                        ffi::LLVMConstInt(i8_ty, 0, 0),
                        cstring("loop.nz")?.as_ptr(),
                    );
                    ffi::LLVMBuildCondBr(self.builder, is_non_zero, body_bb, end_bb);
                    ffi::LLVMPositionBuilderAtEnd(self.builder, body_bb);

                    loop_stack.push((cond_bb, end_bb));
                }
                Op::LoopEnd => {
                    let (cond_bb, end_bb) = loop_stack
                        .pop()
                        .ok_or_else(|| anyhow::anyhow!("internal loop mismatch"))?;
                    ffi::LLVMBuildBr(self.builder, cond_bb);
                    ffi::LLVMPositionBuilderAtEnd(self.builder, end_bb);
                }
            }
        }

        ffi::LLVMBuildRet(self.builder, ffi::LLVMConstInt(i32_ty, 0, 0));

        if !loop_stack.is_empty() {
            anyhow::bail!("internal loop stack not empty");
        }

        if ffi::LLVMVerifyModule(
            self.module,
            ffi::LLVMVerifierFailureAction_LLVMReturnStatusAction,
            std::ptr::null_mut(),
        ) != 0
        {
            anyhow::bail!("LLVM module verification failed");
        }

        Ok(())
    }
    pub fn output(&self, tm: &LLVMTargetMachine) -> anyhow::Result<LLVMMemoryBuffer> {
        let mut buf = LLVMMemoryBuffer::new();
        with_llvm_error(|e| unsafe {
            ffi::LLVMTargetMachineEmitToMemoryBuffer(
                tm.as_raw(),
                self.module,
                ffi::LLVMCodeGenFileType_LLVMObjectFile,
                e,
                buf.as_raw_mut(),
            )
        })
        .map_err(|e| anyhow::anyhow!("failed to emit object file: {e}"))?;
        Ok(buf)
    }
}

impl Drop for LLVMCompiler {
    fn drop(&mut self) {
        unsafe {
            ffi::LLVMDisposeBuilder(self.builder);
            ffi::LLVMDisposeModule(self.module);
            ffi::LLVMContextDispose(self.context);
        }
    }
}

fn run_llvm_passes(
    module: ffi::LLVMModuleRef,
    tm: &LLVMTargetMachine,
    opt_level: &OptLevel,
) -> anyhow::Result<()> {
    let options = PassBuilderOptions::new()?;

    with_llvm_error_ref(unsafe {
        ffi::LLVMRunPasses(
            module,
            cstring(opt_level.llvm_pass_pipeline())?.as_ptr(),
            tm.as_raw(),
            options.as_raw(),
        )
    })
    .map_err(|e| anyhow::anyhow!("failed to run LLVM optimization passes: {e}"))?;

    Ok(())
}

pub fn compile(
    ops: &[Op],
    opt_level: &OptLevel,
    run: bool,
    need_buffer: bool,
) -> anyhow::Result<Option<Vec<u8>>> {
    let compiler = LLVMCompiler::new()?;
    unsafe { compiler.build(ops)? };
    let triple = LLVMTargetTriple::new();

    let target = LLVMTarget::from_triple(&triple)?;
    let tm = LLVMTargetMachine::new(
        &target,
        &triple,
        TARGET_CPU,
        "",
        opt_level.llvm_codegen_level(),
    )?;

    unsafe { ffi::LLVMSetTarget(compiler.module, triple.as_raw()) };

    unsafe {
        let data_layout = ffi::LLVMCreateTargetDataLayout(tm.as_raw());
        let layout_str = ffi::LLVMCopyStringRepOfTargetData(data_layout);
        ffi::LLVMSetDataLayout(compiler.module, layout_str);
        ffi::LLVMDisposeMessage(layout_str);
        ffi::LLVMDisposeTargetData(data_layout);
    }

    run_llvm_passes(compiler.module, &tm, opt_level)?;
    let mut buf = compiler.output(&tm)?;
    let buf_vec = if need_buffer {
        Some(buf.as_slice().to_vec())
    } else {
        None
    };
    if run {
        jit_object(&mut buf)?;
    }
    Ok(buf_vec)
}

pub fn link_executable(
    object_path: &Path,
    output_path: &Path,
    opt_level: &OptLevel,
) -> anyhow::Result<()> {
    let status = Command::new("cc")
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

fn jit_object(obj_buf: &mut LLVMMemoryBuffer) -> anyhow::Result<()> {
    let builder = unsafe { ffi::LLVMOrcCreateLLJITBuilder() };
    if builder.is_null() {
        anyhow::bail!("failed to create LLVM JIT builder");
    }
    let mut jit = std::ptr::null_mut();
    with_llvm_error_ref(unsafe { ffi::LLVMOrcCreateLLJIT(&mut jit, builder) })
        .map_err(|e| anyhow::anyhow!("failed to create LLVM JIT: {e}"))?;
    let dylib = unsafe { ffi::LLVMOrcLLJITGetMainJITDylib(jit) };
    with_llvm_error_ref(unsafe {
        ffi::LLVMOrcLLJITAddObjectFile(jit, dylib, obj_buf.as_raw_mut())
    })
    .map_err(|e| anyhow::anyhow!("failed to add object file to LLVM JIT: {e}"))?;
    let mut addr = 0u64;
    with_llvm_error_ref(unsafe {
        ffi::LLVMOrcLLJITLookup(jit, &mut addr, cstring("main")?.as_ptr())
    })
    .map_err(|e| anyhow::anyhow!("failed to lookup main function in LLVM JIT: {e}"))?;
    let main_fn: extern "C" fn() -> i32 = unsafe { std::mem::transmute(addr) };
    main_fn();
    with_llvm_error_ref(unsafe { ffi::LLVMOrcDisposeLLJIT(jit) })
        .map_err(|e| anyhow::anyhow!("failed to dispose LLVM JIT: {e}"))?;
    Ok(())
}
