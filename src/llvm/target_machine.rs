use crate::llvm::{
    ffi::{self, cstring},
    r#trait::AsRaw,
    target::LLVMTarget,
    triple::LLVMTargetTriple,
};

pub struct LLVMTargetMachine {
    tm: ffi::LLVMTargetMachineRef,
}

impl LLVMTargetMachine {
    pub fn new(
        target: &LLVMTarget,
        triple: &LLVMTargetTriple,
        cpu: &str,
        features: &str,
        opt_level: ffi::LLVMCodeGenOptLevel,
    ) -> anyhow::Result<Self> {
        let tm = unsafe {
            ffi::LLVMCreateTargetMachine(
                target.as_raw(),
                triple.as_raw(),
                cstring(cpu)?.as_ptr() as *mut _,
                cstring(features)?.as_ptr() as *mut _,
                opt_level,
                ffi::LLVMRelocMode_LLVMRelocDefault,
                ffi::LLVMCodeModel_LLVMCodeModelDefault,
            )
        };
        if tm.is_null() {
            anyhow::bail!("failed to create target machine");
        }
        Ok(Self { tm })
    }
}

impl Drop for LLVMTargetMachine {
    fn drop(&mut self) {
        unsafe {
            ffi::LLVMDisposeTargetMachine(self.tm);
        }
    }
}

impl AsRaw<ffi::LLVMTargetMachineRef> for LLVMTargetMachine {
    fn as_raw(&self) -> ffi::LLVMTargetMachineRef {
        self.tm
    }
}
