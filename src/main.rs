use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, ValueEnum};
use std::collections::BTreeMap;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
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

impl OptLevel {
    fn llvm_codegen_level(self) -> llvm::LLVMCodeGenOptLevel {
        match self {
            Self::O0 => llvm::LLVMCodeGenOptLevel_LLVMCodeGenLevelNone,
            Self::O1 => llvm::LLVMCodeGenOptLevel_LLVMCodeGenLevelLess,
            Self::O2 => llvm::LLVMCodeGenOptLevel_LLVMCodeGenLevelDefault,
            Self::O3 => llvm::LLVMCodeGenOptLevel_LLVMCodeGenLevelAggressive,
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

#[derive(Clone, Debug)]
struct CellUpdate {
    offset: i64,
    factor: i32,
}

#[derive(Clone, Debug)]
enum Op {
    PtrAdd(i64),
    CellAdd(i32),
    ClearCell,
    AddScaled(Vec<CellUpdate>),
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
    let mut chars = source.as_bytes().iter().copied().peekable();
    let mut loop_depth = 0usize;

    while let Some(ch) = chars.next() {
        match ch {
            b'>' | b'<' => {
                let mut delta: i64 = if ch == b'>' { 1 } else { -1 };
                while let Some(next) = chars.peek() {
                    if *next == b'>' {
                        delta += 1;
                        chars.next();
                    } else if *next == b'<' {
                        delta -= 1;
                        chars.next();
                    } else {
                        break;
                    }
                }
                push_op(&mut ops, Op::PtrAdd(delta));
            }
            b'+' | b'-' => {
                let mut delta: i32 = if ch == b'+' { 1 } else { -1 };
                while let Some(next) = chars.peek() {
                    if *next == b'+' {
                        delta += 1;
                        chars.next();
                    } else if *next == b'-' {
                        delta -= 1;
                        chars.next();
                    } else {
                        break;
                    }
                }
                push_op(&mut ops, Op::CellAdd(delta));
            }
            b'.' => ops.push(Op::Output),
            b',' => ops.push(Op::Input),
            b'[' => {
                loop_depth += 1;
                ops.push(Op::LoopStart);
            }
            b']' => {
                if loop_depth == 0 {
                    bail!("unmatched closing bracket ']' found");
                }
                loop_depth -= 1;
                ops.push(Op::LoopEnd);
            }
            _ => {}
        }
    }

    if loop_depth != 0 {
        bail!("unmatched opening bracket '[' found");
    }

    optimize_ops(&ops)
}

fn push_op(ops: &mut Vec<Op>, op: Op) {
    match op {
        Op::PtrAdd(0) | Op::CellAdd(0) => {}
        Op::PtrAdd(delta) => {
            if let Some(Op::PtrAdd(prev)) = ops.last_mut() {
                *prev += delta;
                if *prev == 0 {
                    ops.pop();
                }
            } else {
                ops.push(Op::PtrAdd(delta));
            }
        }
        Op::CellAdd(delta) => {
            if let Some(Op::CellAdd(prev)) = ops.last_mut() {
                *prev += delta;
                if *prev == 0 {
                    ops.pop();
                }
            } else {
                ops.push(Op::CellAdd(delta));
            }
        }
        other => ops.push(other),
    }
}

fn optimize_ops(ops: &[Op]) -> Result<Vec<Op>> {
    let loop_pairs = compute_loop_pairs(ops)?;
    optimize_range(ops, &loop_pairs, 0, ops.len())
}

fn compute_loop_pairs(ops: &[Op]) -> Result<Vec<usize>> {
    let mut loop_pairs = vec![usize::MAX; ops.len()];
    let mut stack = Vec::new();

    for (index, op) in ops.iter().enumerate() {
        match op {
            Op::LoopStart => stack.push(index),
            Op::LoopEnd => {
                let start = stack
                    .pop()
                    .ok_or_else(|| anyhow!("internal unmatched closing bracket"))?;
                loop_pairs[start] = index;
                loop_pairs[index] = start;
            }
            _ => {}
        }
    }

    if !stack.is_empty() {
        bail!("internal unmatched opening bracket");
    }

    Ok(loop_pairs)
}

fn optimize_range(ops: &[Op], loop_pairs: &[usize], start: usize, end: usize) -> Result<Vec<Op>> {
    let mut optimized = Vec::new();
    let mut index = start;

    while index < end {
        match &ops[index] {
            Op::LoopStart => {
                let loop_end = loop_pairs[index];
                let body = optimize_range(ops, loop_pairs, index + 1, loop_end)?;
                if let Some(op) = try_optimize_loop(&body) {
                    push_op(&mut optimized, op);
                } else {
                    optimized.push(Op::LoopStart);
                    optimized.extend(body);
                    optimized.push(Op::LoopEnd);
                }
                index = loop_end + 1;
            }
            Op::LoopEnd => bail!("internal unexpected loop terminator"),
            other => {
                push_op(&mut optimized, other.clone());
                index += 1;
            }
        }
    }

    Ok(optimized)
}

fn try_optimize_loop(body: &[Op]) -> Option<Op> {
    try_optimize_clear_loop(body).or_else(|| try_optimize_add_scaled_loop(body))
}

fn try_optimize_clear_loop(body: &[Op]) -> Option<Op> {
    match body {
        [Op::CellAdd(delta)] if delta.rem_euclid(2) != 0 => Some(Op::ClearCell),
        _ => None,
    }
}

fn try_optimize_add_scaled_loop(body: &[Op]) -> Option<Op> {
    let mut pointer_offset = 0i64;
    let mut current_delta = 0i32;
    let mut updates = BTreeMap::new();

    for op in body {
        match op {
            Op::PtrAdd(delta) => pointer_offset += delta,
            Op::CellAdd(delta) => {
                if pointer_offset == 0 {
                    current_delta += delta;
                } else {
                    *updates.entry(pointer_offset).or_insert(0) += delta;
                }
            }
            _ => return None,
        }
    }

    if pointer_offset != 0 || current_delta.rem_euclid(256) != 255 {
        return None;
    }

    let updates: Vec<_> = updates
        .into_iter()
        .filter_map(|(offset, factor)| {
            let wrapped = factor.rem_euclid(256);
            (wrapped != 0).then_some(CellUpdate { offset, factor })
        })
        .collect();

    if updates.is_empty() {
        Some(Op::ClearCell)
    } else {
        Some(Op::AddScaled(updates))
    }
}

unsafe fn llvm_error_to_string(err: *mut c_char) -> String {
    if err.is_null() {
        return "unknown LLVM error".to_string();
    }
    let msg = CStr::from_ptr(err).to_string_lossy().to_string();
    llvm::LLVMDisposeMessage(err);
    msg
}

unsafe fn llvm_error_ref_to_string(err: llvm::LLVMErrorRef) -> String {
    if err.is_null() {
        return "unknown LLVM error".to_string();
    }
    let msg = llvm::LLVMGetErrorMessage(err);
    let text = CStr::from_ptr(msg).to_string_lossy().to_string();
    llvm::LLVMDisposeErrorMessage(msg);
    text
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

    unsafe fn build_cell_ptr(
        &self,
        tape_ty: llvm::LLVMTypeRef,
        tape: llvm::LLVMValueRef,
        idx: llvm::LLVMValueRef,
        zero_i64: llvm::LLVMValueRef,
        name: &str,
    ) -> Result<llvm::LLVMValueRef> {
        Ok(llvm::LLVMBuildInBoundsGEP2(
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
        tape_ty: llvm::LLVMTypeRef,
        tape: llvm::LLVMValueRef,
        idx_ptr: llvm::LLVMValueRef,
        i64_ty: llvm::LLVMTypeRef,
        zero_i64: llvm::LLVMValueRef,
        idx_name: &str,
        ptr_name: &str,
    ) -> Result<(llvm::LLVMValueRef, llvm::LLVMValueRef)> {
        let cur_idx =
            llvm::LLVMBuildLoad2(self.builder, i64_ty, idx_ptr, cstring(idx_name)?.as_ptr());
        let cell_ptr = self.build_cell_ptr(tape_ty, tape, cur_idx, zero_i64, ptr_name)?;
        Ok((cur_idx, cell_ptr))
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
            match op {
                Op::PtrAdd(delta) => {
                    let cur = llvm::LLVMBuildLoad2(
                        self.builder,
                        i64_ty,
                        idx_ptr,
                        cstring("idx.cur")?.as_ptr(),
                    );
                    let val = llvm::LLVMConstInt(i64_ty, delta.unsigned_abs(), 0);
                    let next = if *delta >= 0 {
                        llvm::LLVMBuildAdd(self.builder, cur, val, cstring("idx.add")?.as_ptr())
                    } else {
                        llvm::LLVMBuildSub(self.builder, cur, val, cstring("idx.sub")?.as_ptr())
                    };
                    llvm::LLVMBuildStore(self.builder, next, idx_ptr);
                }
                Op::CellAdd(delta) => {
                    let (_, cell_ptr) = self.build_current_cell_ptr(
                        tape_ty, tape, idx_ptr, i64_ty, zero_i64, "idx.load", "cell.ptr",
                    )?;
                    let cur_val = llvm::LLVMBuildLoad2(
                        self.builder,
                        i8_ty,
                        cell_ptr,
                        cstring("cell.cur")?.as_ptr(),
                    );
                    let delta_val =
                        llvm::LLVMConstInt(i8_ty, (*delta as i64).rem_euclid(256) as u64, 0);
                    let next = llvm::LLVMBuildAdd(
                        self.builder,
                        cur_val,
                        delta_val,
                        cstring("cell.next")?.as_ptr(),
                    );
                    llvm::LLVMBuildStore(self.builder, next, cell_ptr);
                }
                Op::ClearCell => {
                    let (_, cell_ptr) = self.build_current_cell_ptr(
                        tape_ty, tape, idx_ptr, i64_ty, zero_i64, "idx.load", "cell.ptr",
                    )?;
                    llvm::LLVMBuildStore(self.builder, llvm::LLVMConstInt(i8_ty, 0, 0), cell_ptr);
                }
                Op::AddScaled(updates) => {
                    let (cur_idx, cell_ptr) = self.build_current_cell_ptr(
                        tape_ty, tape, idx_ptr, i64_ty, zero_i64, "idx.load", "cell.ptr",
                    )?;
                    let cell = llvm::LLVMBuildLoad2(
                        self.builder,
                        i8_ty,
                        cell_ptr,
                        cstring("cell.mul")?.as_ptr(),
                    );

                    for update in updates {
                        let offset = llvm::LLVMConstInt(i64_ty, update.offset.unsigned_abs(), 0);
                        let target_idx = if update.offset >= 0 {
                            llvm::LLVMBuildAdd(
                                self.builder,
                                cur_idx,
                                offset,
                                cstring("idx.scaled.add")?.as_ptr(),
                            )
                        } else {
                            llvm::LLVMBuildSub(
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
                        let target = llvm::LLVMBuildLoad2(
                            self.builder,
                            i8_ty,
                            target_ptr,
                            cstring("cell.scaled.cur")?.as_ptr(),
                        );
                        let scale = llvm::LLVMConstInt(
                            i8_ty,
                            (update.factor as i64).rem_euclid(256) as u64,
                            0,
                        );
                        let scaled = llvm::LLVMBuildMul(
                            self.builder,
                            cell,
                            scale,
                            cstring("cell.scaled.mul")?.as_ptr(),
                        );
                        let next = llvm::LLVMBuildAdd(
                            self.builder,
                            target,
                            scaled,
                            cstring("cell.scaled.next")?.as_ptr(),
                        );
                        llvm::LLVMBuildStore(self.builder, next, target_ptr);
                    }

                    llvm::LLVMBuildStore(self.builder, llvm::LLVMConstInt(i8_ty, 0, 0), cell_ptr);
                }
                Op::Output => {
                    let (_, cell_ptr) = self.build_current_cell_ptr(
                        tape_ty, tape, idx_ptr, i64_ty, zero_i64, "idx.load", "cell.ptr",
                    )?;
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
                    let (_, cell_ptr) = self.build_current_cell_ptr(
                        tape_ty, tape, idx_ptr, i64_ty, zero_i64, "idx.load", "cell.ptr",
                    )?;
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

                    let (_, cell_ptr) = self.build_current_cell_ptr(
                        tape_ty, tape, idx_ptr, i64_ty, zero_i64, "idx.loop", "cell.ptr",
                    )?;
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

unsafe fn run_llvm_passes(
    module: llvm::LLVMModuleRef,
    target_machine: llvm::LLVMTargetMachineRef,
    opt_level: OptLevel,
) -> Result<()> {
    let pass_pipeline = cstring(opt_level.llvm_pass_pipeline())?;
    let options = llvm::LLVMCreatePassBuilderOptions();
    if options.is_null() {
        bail!("failed to create LLVM pass builder options");
    }

    let err = llvm::LLVMRunPasses(module, pass_pipeline.as_ptr(), target_machine, options);
    llvm::LLVMDisposePassBuilderOptions(options);

    if !err.is_null() {
        bail!(
            "failed to run LLVM optimization passes: {}",
            llvm_error_ref_to_string(err)
        );
    }

    Ok(())
}

unsafe fn compile_to_object(ops: &[Op], object_path: &Path, opt_level: &OptLevel) -> Result<()> {
    let compiler = LLVMCompiler::new()?;
    compiler.build(ops)?;
    let triple = LLVMTriple::new();

    let mut target = std::ptr::null_mut();
    let mut target_err = std::ptr::null_mut();
    if llvm::LLVMGetTargetFromTriple(triple.triple, &mut target, &mut target_err) != 0 {
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

    llvm::LLVMSetTarget(compiler.module, triple.triple);

    let data_layout = llvm::LLVMCreateTargetDataLayout(tm.tm);
    let layout_str = llvm::LLVMCopyStringRepOfTargetData(data_layout);
    llvm::LLVMSetDataLayout(compiler.module, layout_str);
    llvm::LLVMDisposeMessage(layout_str);
    llvm::LLVMDisposeTargetData(data_layout);

    run_llvm_passes(compiler.module, tm.tm, *opt_level)?;

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
        .arg(opt_level.cc_opt_flag())
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
