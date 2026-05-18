// llvm target doesn't need dispose
use crate::llvm::{AsRaw, LLVMTargetTriple, ffi};

pub struct LLVMTarget {
    target: ffi::LLVMTargetRef,
}

impl LLVMTarget {
    pub fn from_triple(triple: &LLVMTargetTriple) -> anyhow::Result<Self> {
        let mut target = std::ptr::null_mut();
        ffi::with_llvm_error(|e| unsafe {
            ffi::LLVMGetTargetFromTriple(triple.as_raw(), &mut target, e)
        })
        .map_err(|e| anyhow::anyhow!("failed to get LLVM target from triple: {e}"))?;
        Ok(Self { target })
    }
}

impl AsRaw<ffi::LLVMTargetRef> for LLVMTarget {
    fn as_raw(&self) -> ffi::LLVMTargetRef {
        self.target
    }
}
