use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use std::ffi::{CStr, CString};
use std::fs;
use std::os::raw::c_char;
use std::path::{Path, PathBuf};
use std::process::Command;

mod llvm {
    #![allow(dead_code)]
    #![allow(non_camel_case_types)]
    #![allow(non_snake_case)]
    #![allow(non_upper_case_globals)]
    #![allow(clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/llvm_bindings.rs"));
}

#[derive(Parser, Debug)]
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

const MODULE_NAME: &str = "bf_module";
const PUTCHAR_NAME: &str = "putchar";
const GETCHAR_NAME: &str = "getchar";
const MAIN_FN_NAME: &str = "main";
const ENTRY_BLOCK_NAME: &str = "entry";
const TAPE_NAME: &str = "tape";
const INDEX_NAME: &str = "idx";
const TARGET_CPU: &str = "generic";
const TAPE_LEN: u64 = 30_000;

type LlvmInitFn = unsafe extern "C" fn();

const LLVM_TARGET_INFO_INITS: &[LlvmInitFn] = &[
    llvm::LLVMInitializeAArch64TargetInfo,
    llvm::LLVMInitializeAMDGPUTargetInfo,
    llvm::LLVMInitializeARMTargetInfo,
    llvm::LLVMInitializeAVRTargetInfo,
    llvm::LLVMInitializeBPFTargetInfo,
    llvm::LLVMInitializeHexagonTargetInfo,
    llvm::LLVMInitializeLanaiTargetInfo,
    llvm::LLVMInitializeLoongArchTargetInfo,
    llvm::LLVMInitializeMipsTargetInfo,
    llvm::LLVMInitializeMSP430TargetInfo,
    llvm::LLVMInitializeNVPTXTargetInfo,
    llvm::LLVMInitializePowerPCTargetInfo,
    llvm::LLVMInitializeRISCVTargetInfo,
    llvm::LLVMInitializeSparcTargetInfo,
    llvm::LLVMInitializeSystemZTargetInfo,
    llvm::LLVMInitializeVETargetInfo,
    llvm::LLVMInitializeWebAssemblyTargetInfo,
    llvm::LLVMInitializeX86TargetInfo,
    llvm::LLVMInitializeXCoreTargetInfo,
];

const LLVM_TARGET_INITS: &[LlvmInitFn] = &[
    llvm::LLVMInitializeAArch64Target,
    llvm::LLVMInitializeAMDGPUTarget,
    llvm::LLVMInitializeARMTarget,
    llvm::LLVMInitializeAVRTarget,
    llvm::LLVMInitializeBPFTarget,
    llvm::LLVMInitializeHexagonTarget,
    llvm::LLVMInitializeLanaiTarget,
    llvm::LLVMInitializeLoongArchTarget,
    llvm::LLVMInitializeMipsTarget,
    llvm::LLVMInitializeMSP430Target,
    llvm::LLVMInitializeNVPTXTarget,
    llvm::LLVMInitializePowerPCTarget,
    llvm::LLVMInitializeRISCVTarget,
    llvm::LLVMInitializeSparcTarget,
    llvm::LLVMInitializeSystemZTarget,
    llvm::LLVMInitializeVETarget,
    llvm::LLVMInitializeWebAssemblyTarget,
    llvm::LLVMInitializeX86Target,
    llvm::LLVMInitializeXCoreTarget,
];

const LLVM_TARGET_MC_INITS: &[LlvmInitFn] = &[
    llvm::LLVMInitializeAArch64TargetMC,
    llvm::LLVMInitializeAMDGPUTargetMC,
    llvm::LLVMInitializeARMTargetMC,
    llvm::LLVMInitializeAVRTargetMC,
    llvm::LLVMInitializeBPFTargetMC,
    llvm::LLVMInitializeHexagonTargetMC,
    llvm::LLVMInitializeLanaiTargetMC,
    llvm::LLVMInitializeLoongArchTargetMC,
    llvm::LLVMInitializeMipsTargetMC,
    llvm::LLVMInitializeMSP430TargetMC,
    llvm::LLVMInitializeNVPTXTargetMC,
    llvm::LLVMInitializePowerPCTargetMC,
    llvm::LLVMInitializeRISCVTargetMC,
    llvm::LLVMInitializeSparcTargetMC,
    llvm::LLVMInitializeSystemZTargetMC,
    llvm::LLVMInitializeVETargetMC,
    llvm::LLVMInitializeWebAssemblyTargetMC,
    llvm::LLVMInitializeX86TargetMC,
    llvm::LLVMInitializeXCoreTargetMC,
];

const LLVM_ASM_PRINTER_INITS: &[LlvmInitFn] = &[
    llvm::LLVMInitializeAArch64AsmPrinter,
    llvm::LLVMInitializeAMDGPUAsmPrinter,
    llvm::LLVMInitializeARMAsmPrinter,
    llvm::LLVMInitializeAVRAsmPrinter,
    llvm::LLVMInitializeBPFAsmPrinter,
    llvm::LLVMInitializeHexagonAsmPrinter,
    llvm::LLVMInitializeLanaiAsmPrinter,
    llvm::LLVMInitializeLoongArchAsmPrinter,
    llvm::LLVMInitializeMipsAsmPrinter,
    llvm::LLVMInitializeMSP430AsmPrinter,
    llvm::LLVMInitializeNVPTXAsmPrinter,
    llvm::LLVMInitializePowerPCAsmPrinter,
    llvm::LLVMInitializeRISCVAsmPrinter,
    llvm::LLVMInitializeSparcAsmPrinter,
    llvm::LLVMInitializeSystemZAsmPrinter,
    llvm::LLVMInitializeVEAsmPrinter,
    llvm::LLVMInitializeWebAssemblyAsmPrinter,
    llvm::LLVMInitializeX86AsmPrinter,
    llvm::LLVMInitializeXCoreAsmPrinter,
];

const LLVM_ASM_PARSER_INITS: &[LlvmInitFn] = &[
    llvm::LLVMInitializeAArch64AsmParser,
    llvm::LLVMInitializeAMDGPUAsmParser,
    llvm::LLVMInitializeARMAsmParser,
    llvm::LLVMInitializeAVRAsmParser,
    llvm::LLVMInitializeBPFAsmParser,
    llvm::LLVMInitializeHexagonAsmParser,
    llvm::LLVMInitializeLanaiAsmParser,
    llvm::LLVMInitializeLoongArchAsmParser,
    llvm::LLVMInitializeMipsAsmParser,
    llvm::LLVMInitializeMSP430AsmParser,
    llvm::LLVMInitializePowerPCAsmParser,
    llvm::LLVMInitializeRISCVAsmParser,
    llvm::LLVMInitializeSparcAsmParser,
    llvm::LLVMInitializeSystemZAsmParser,
    llvm::LLVMInitializeVEAsmParser,
    llvm::LLVMInitializeWebAssemblyAsmParser,
    llvm::LLVMInitializeX86AsmParser,
];

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
    for init in LLVM_TARGET_INFO_INITS {
        init();
    }
    for init in LLVM_TARGET_INITS {
        init();
    }
    for init in LLVM_TARGET_MC_INITS {
        init();
    }
    for init in LLVM_ASM_PRINTER_INITS {
        init();
    }
    for init in LLVM_ASM_PARSER_INITS {
        init();
    }
}

fn compile_to_object(ops: &[Op], object_path: &Path) -> Result<()> {
    unsafe {
        initialize_llvm_targets();

        let ctx = llvm::LLVMContextCreate();
        if ctx.is_null() {
            bail!("failed to create LLVM context");
        }

        let module_name = cstring(MODULE_NAME)?;
        let module = llvm::LLVMModuleCreateWithNameInContext(module_name.as_ptr(), ctx);
        let builder = llvm::LLVMCreateBuilderInContext(ctx);

        let i8_ty = llvm::LLVMInt8TypeInContext(ctx);
        let i32_ty = llvm::LLVMInt32TypeInContext(ctx);
        let i64_ty = llvm::LLVMInt64TypeInContext(ctx);
        let putchar_ty = llvm::LLVMFunctionType(i32_ty, [i32_ty].as_ptr().cast_mut(), 1, 0);
        let getchar_ty = llvm::LLVMFunctionType(i32_ty, std::ptr::null_mut(), 0, 0);
        let putchar_name = cstring(PUTCHAR_NAME)?;
        let getchar_name = cstring(GETCHAR_NAME)?;
        let putchar_fn = llvm::LLVMAddFunction(module, putchar_name.as_ptr(), putchar_ty);
        let getchar_fn = llvm::LLVMAddFunction(module, getchar_name.as_ptr(), getchar_ty);

        let main_ty = llvm::LLVMFunctionType(i32_ty, std::ptr::null_mut(), 0, 0);
        let main_name = cstring(MAIN_FN_NAME)?;
        let main_fn = llvm::LLVMAddFunction(module, main_name.as_ptr(), main_ty);
        let entry_name = cstring(ENTRY_BLOCK_NAME)?;
        let entry = llvm::LLVMAppendBasicBlockInContext(ctx, main_fn, entry_name.as_ptr());
        llvm::LLVMPositionBuilderAtEnd(builder, entry);

        let tape_ty = llvm::LLVMArrayType2(i8_ty, TAPE_LEN);
        let tape_name = cstring(TAPE_NAME)?;
        let tape = llvm::LLVMBuildAlloca(builder, tape_ty, tape_name.as_ptr());
        llvm::LLVMBuildMemSet(
            builder,
            tape,
            llvm::LLVMConstInt(i8_ty, 0, 0),
            llvm::LLVMConstInt(i64_ty, TAPE_LEN, 0),
            1,
        );

        let idx_name = cstring(INDEX_NAME)?;
        let idx_ptr = llvm::LLVMBuildAlloca(builder, i64_ty, idx_name.as_ptr());
        llvm::LLVMBuildStore(builder, llvm::LLVMConstInt(i64_ty, 0, 0), idx_ptr);

        let zero_i64 = llvm::LLVMConstInt(i64_ty, 0, 0);

        let mut loop_stack: Vec<(llvm::LLVMBasicBlockRef, llvm::LLVMBasicBlockRef)> = Vec::new();

        for op in ops {
            match *op {
                Op::PtrAdd(delta) => {
                    let cur = llvm::LLVMBuildLoad2(
                        builder,
                        i64_ty,
                        idx_ptr,
                        cstring("idx.cur")?.as_ptr(),
                    );
                    let val = llvm::LLVMConstInt(i64_ty, delta.unsigned_abs(), 0);
                    let next = if delta >= 0 {
                        llvm::LLVMBuildAdd(builder, cur, val, cstring("idx.add")?.as_ptr())
                    } else {
                        llvm::LLVMBuildSub(builder, cur, val, cstring("idx.sub")?.as_ptr())
                    };
                    llvm::LLVMBuildStore(builder, next, idx_ptr);
                }
                Op::CellAdd(delta) => {
                    let cur_idx = llvm::LLVMBuildLoad2(
                        builder,
                        i64_ty,
                        idx_ptr,
                        cstring("idx.load")?.as_ptr(),
                    );
                    let cell_ptr = llvm::LLVMBuildInBoundsGEP2(
                        builder,
                        tape_ty,
                        tape,
                        [zero_i64, cur_idx].as_ptr().cast_mut(),
                        2,
                        cstring("cell.ptr")?.as_ptr(),
                    );
                    let cur_val = llvm::LLVMBuildLoad2(
                        builder,
                        i8_ty,
                        cell_ptr,
                        cstring("cell.cur")?.as_ptr(),
                    );
                    let delta_val =
                        llvm::LLVMConstInt(i8_ty, (delta as i64).rem_euclid(256) as u64, 0);
                    let next = llvm::LLVMBuildAdd(
                        builder,
                        cur_val,
                        delta_val,
                        cstring("cell.next")?.as_ptr(),
                    );
                    llvm::LLVMBuildStore(builder, next, cell_ptr);
                }
                Op::Output => {
                    let cur_idx = llvm::LLVMBuildLoad2(
                        builder,
                        i64_ty,
                        idx_ptr,
                        cstring("idx.load")?.as_ptr(),
                    );
                    let cell_ptr = llvm::LLVMBuildInBoundsGEP2(
                        builder,
                        tape_ty,
                        tape,
                        [zero_i64, cur_idx].as_ptr().cast_mut(),
                        2,
                        cstring("cell.ptr")?.as_ptr(),
                    );
                    let cell = llvm::LLVMBuildLoad2(
                        builder,
                        i8_ty,
                        cell_ptr,
                        cstring("cell.out")?.as_ptr(),
                    );
                    let widened =
                        llvm::LLVMBuildZExt(builder, cell, i32_ty, cstring("out.zext")?.as_ptr());
                    llvm::LLVMBuildCall2(
                        builder,
                        putchar_ty,
                        putchar_fn,
                        [widened].as_ptr().cast_mut(),
                        1,
                        cstring("")?.as_ptr(),
                    );
                }
                Op::Input => {
                    let input = llvm::LLVMBuildCall2(
                        builder,
                        getchar_ty,
                        getchar_fn,
                        std::ptr::null_mut(),
                        0,
                        cstring("in")?.as_ptr(),
                    );
                    let byte =
                        llvm::LLVMBuildTrunc(builder, input, i8_ty, cstring("in.byte")?.as_ptr());
                    let cur_idx = llvm::LLVMBuildLoad2(
                        builder,
                        i64_ty,
                        idx_ptr,
                        cstring("idx.load")?.as_ptr(),
                    );
                    let cell_ptr = llvm::LLVMBuildInBoundsGEP2(
                        builder,
                        tape_ty,
                        tape,
                        [zero_i64, cur_idx].as_ptr().cast_mut(),
                        2,
                        cstring("cell.ptr")?.as_ptr(),
                    );
                    llvm::LLVMBuildStore(builder, byte, cell_ptr);
                }
                Op::LoopStart => {
                    let cond_bb = llvm::LLVMAppendBasicBlockInContext(
                        ctx,
                        main_fn,
                        cstring("loop.cond")?.as_ptr(),
                    );
                    let body_bb = llvm::LLVMAppendBasicBlockInContext(
                        ctx,
                        main_fn,
                        cstring("loop.body")?.as_ptr(),
                    );
                    let end_bb = llvm::LLVMAppendBasicBlockInContext(
                        ctx,
                        main_fn,
                        cstring("loop.end")?.as_ptr(),
                    );

                    llvm::LLVMBuildBr(builder, cond_bb);
                    llvm::LLVMPositionBuilderAtEnd(builder, cond_bb);

                    let cur_idx = llvm::LLVMBuildLoad2(
                        builder,
                        i64_ty,
                        idx_ptr,
                        cstring("idx.loop")?.as_ptr(),
                    );
                    let cell_ptr = llvm::LLVMBuildInBoundsGEP2(
                        builder,
                        tape_ty,
                        tape,
                        [zero_i64, cur_idx].as_ptr().cast_mut(),
                        2,
                        cstring("cell.ptr")?.as_ptr(),
                    );
                    let cell = llvm::LLVMBuildLoad2(
                        builder,
                        i8_ty,
                        cell_ptr,
                        cstring("cell.loop")?.as_ptr(),
                    );
                    let is_non_zero = llvm::LLVMBuildICmp(
                        builder,
                        llvm::LLVMIntPredicate_LLVMIntNE,
                        cell,
                        llvm::LLVMConstInt(i8_ty, 0, 0),
                        cstring("loop.nz")?.as_ptr(),
                    );
                    llvm::LLVMBuildCondBr(builder, is_non_zero, body_bb, end_bb);
                    llvm::LLVMPositionBuilderAtEnd(builder, body_bb);

                    loop_stack.push((cond_bb, end_bb));
                }
                Op::LoopEnd => {
                    let (cond_bb, end_bb) = loop_stack
                        .pop()
                        .ok_or_else(|| anyhow!("internal loop mismatch"))?;
                    llvm::LLVMBuildBr(builder, cond_bb);
                    llvm::LLVMPositionBuilderAtEnd(builder, end_bb);
                }
            }
        }

        llvm::LLVMBuildRet(builder, llvm::LLVMConstInt(i32_ty, 0, 0));

        if !loop_stack.is_empty() {
            bail!("internal loop stack not empty");
        }

        if llvm::LLVMVerifyModule(
            module,
            llvm::LLVMVerifierFailureAction_LLVMReturnStatusAction,
            std::ptr::null_mut(),
        ) != 0
        {
            bail!("LLVM module verification failed");
        }

        let triple = llvm::LLVMGetDefaultTargetTriple();
        if triple.is_null() {
            bail!("failed to get target triple");
        }

        let mut target = std::ptr::null_mut();
        let mut target_err = std::ptr::null_mut();
        if llvm::LLVMGetTargetFromTriple(triple, &mut target, &mut target_err) != 0 {
            let msg = llvm_error_to_string(target_err);
            llvm::LLVMDisposeMessage(triple);
            bail!("failed to get target from triple: {msg}");
        }

        let cpu = cstring(TARGET_CPU)?;
        let features = cstring("")?;
        let tm = llvm::LLVMCreateTargetMachine(
            target,
            triple,
            cpu.as_ptr(),
            features.as_ptr(),
            llvm::LLVMCodeGenOptLevel_LLVMCodeGenLevelDefault,
            llvm::LLVMRelocMode_LLVMRelocDefault,
            llvm::LLVMCodeModel_LLVMCodeModelDefault,
        );
        if tm.is_null() {
            llvm::LLVMDisposeMessage(triple);
            bail!("failed to create target machine");
        }

        llvm::LLVMSetTarget(module, triple);

        let data_layout = llvm::LLVMCreateTargetDataLayout(tm);
        let layout_str = llvm::LLVMCopyStringRepOfTargetData(data_layout);
        llvm::LLVMSetDataLayout(module, layout_str);
        llvm::LLVMDisposeMessage(layout_str);
        llvm::LLVMDisposeTargetData(data_layout);

        let mut emit_err = std::ptr::null_mut();
        let object_c = cstring(&object_path.to_string_lossy())?;
        if llvm::LLVMTargetMachineEmitToFile(
            tm,
            module,
            object_c.as_ptr().cast_mut(),
            llvm::LLVMCodeGenFileType_LLVMObjectFile,
            &mut emit_err,
        ) != 0
        {
            let msg = llvm_error_to_string(emit_err);
            llvm::LLVMDisposeTargetMachine(tm);
            llvm::LLVMDisposeMessage(triple);
            llvm::LLVMDisposeBuilder(builder);
            llvm::LLVMDisposeModule(module);
            llvm::LLVMContextDispose(ctx);
            bail!("failed to emit object file: {msg}");
        }

        llvm::LLVMDisposeTargetMachine(tm);
        llvm::LLVMDisposeMessage(triple);
        llvm::LLVMDisposeBuilder(builder);
        llvm::LLVMDisposeModule(module);
        llvm::LLVMContextDispose(ctx);
    }

    Ok(())
}

fn link_executable(object_path: &Path, output_path: &Path) -> Result<()> {
    let status = Command::new("cc")
        .arg(object_path)
        .arg("-o")
        .arg(output_path)
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
    compile_to_object(&ops, &object_path)?;
    link_executable(&object_path, &cli.output)?;

    if !cli.keep_obj {
        let _ = fs::remove_file(&object_path);
    }

    println!("generated executable: {}", cli.output.display());

    Ok(())
}
