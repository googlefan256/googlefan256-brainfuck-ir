#![allow(non_upper_case_globals)]
#![allow(dead_code)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
include!(concat!(env!("OUT_DIR"), "/llvm_bindings.rs"));

use std::{
    ffi::{CStr, CString},
    sync::atomic::{AtomicBool, Ordering},
};

fn from_ptr_string(ptr: *const i8) -> String {
    unsafe { CStr::from_ptr(ptr) }.to_string_lossy().to_string()
}

pub fn with_llvm_error_ref(err: LLVMErrorRef) -> anyhow::Result<()> {
    if !err.is_null() {
        let msg = unsafe { LLVMGetErrorMessage(err) };
        let text = from_ptr_string(msg);
        unsafe { LLVMDisposeErrorMessage(msg) };
        anyhow::bail!("{text}");
    }
    Ok(())
}

pub fn with_llvm_error(f: impl FnOnce(*mut *mut i8) -> i32) -> anyhow::Result<()> {
    let mut err = std::ptr::null_mut();
    if f(&mut err) != 0 {
        if err.is_null() {
            anyhow::bail!("unknown LLVM error");
        }
        let msg = from_ptr_string(err);
        unsafe { LLVMDisposeMessage(err) };
        anyhow::bail!("{msg}");
    }
    Ok(())
}

pub fn cstring(s: &str) -> anyhow::Result<CString> {
    CString::new(s).map_err(|_| anyhow::anyhow!("string contains interior NUL: {s:?}"))
}

static ONCE_FN: AtomicBool = AtomicBool::new(false);

fn initialize_llvm_targets() {
    unsafe {
        LLVMInitializeAllTargetInfosShim();
        LLVMInitializeAllTargetsShim();
        LLVMInitializeAllTargetMCsShim();
        LLVMInitializeAllAsmPrintersShim();
        LLVMInitializeAllAsmParsersShim();
    }
}

pub fn init() {
    if !ONCE_FN.swap(true, Ordering::AcqRel) {
        initialize_llvm_targets();
    }
}
