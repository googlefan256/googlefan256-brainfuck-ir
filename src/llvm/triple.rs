use core::ffi::c_char;

use crate::llvm::{ffi, r#trait::AsRaw};

pub struct LLVMTargetTriple {
    triple: *mut c_char,
}

impl LLVMTargetTriple {
    pub fn new() -> Self {
        Self {
            triple: unsafe { ffi::LLVMGetDefaultTargetTriple() },
        }
    }
}

impl Drop for LLVMTargetTriple {
    fn drop(&mut self) {
        unsafe {
            ffi::LLVMDisposeMessage(self.triple);
        }
    }
}

impl AsRaw<*mut c_char> for LLVMTargetTriple {
    fn as_raw(&self) -> *mut c_char {
        self.triple
    }
}
