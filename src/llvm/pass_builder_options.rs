use crate::llvm::{AsRaw, ffi};

pub struct PassBuilderOptions {
    options: ffi::LLVMPassBuilderOptionsRef,
}

impl PassBuilderOptions {
    pub fn new() -> anyhow::Result<Self> {
        let options = unsafe { ffi::LLVMCreatePassBuilderOptions() };
        if options.is_null() {
            anyhow::bail!("failed to create LLVM pass builder options");
        }
        Ok(Self { options })
    }
}

impl AsRaw<ffi::LLVMPassBuilderOptionsRef> for PassBuilderOptions {
    fn as_raw(&self) -> ffi::LLVMPassBuilderOptionsRef {
        self.options
    }
}

impl Drop for PassBuilderOptions {
    fn drop(&mut self) {
        unsafe {
            ffi::LLVMDisposePassBuilderOptions(self.options);
        }
    }
}
