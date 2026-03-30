use crate::llvm::{ffi, r#trait::AsRawMut};

pub struct LLVMMemoryBuffer {
    buf: ffi::LLVMMemoryBufferRef,
    taken_marker: bool,
}

impl LLVMMemoryBuffer {
    pub fn new() -> Self {
        Self {
            buf: std::ptr::null_mut(),
            taken_marker: false,
        }
    }

    pub fn as_slice(&self) -> &[u8] {
        unsafe {
            let ptr = ffi::LLVMGetBufferStart(self.buf) as *const u8;
            let len = ffi::LLVMGetBufferSize(self.buf);
            std::slice::from_raw_parts(ptr, len)
        }
    }
}

impl AsRawMut<ffi::LLVMMemoryBufferRef> for LLVMMemoryBuffer {
    fn as_raw_mut(&mut self) -> ffi::LLVMMemoryBufferRef {
        self.taken_marker = true;
        self.buf
    }
}

impl AsRawMut<*mut ffi::LLVMMemoryBufferRef> for LLVMMemoryBuffer {
    fn as_raw_mut(&mut self) -> *mut ffi::LLVMMemoryBufferRef {
        &mut self.buf
    }
}

impl Drop for LLVMMemoryBuffer {
    fn drop(&mut self) {
        if !self.taken_marker && !self.buf.is_null() {
            unsafe {
                ffi::LLVMDisposeMemoryBuffer(self.buf);
            }
        }
    }
}
