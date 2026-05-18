use super::ffi;

pub enum OptLevel {
    O0,
    O1,
    O2,
    O3,
}

impl OptLevel {
    pub(super) fn llvm_codegen_level(&self) -> ffi::LLVMCodeGenOptLevel {
        match self {
            Self::O0 => ffi::LLVMCodeGenOptLevel_LLVMCodeGenLevelNone,
            Self::O1 => ffi::LLVMCodeGenOptLevel_LLVMCodeGenLevelLess,
            Self::O2 => ffi::LLVMCodeGenOptLevel_LLVMCodeGenLevelDefault,
            Self::O3 => ffi::LLVMCodeGenOptLevel_LLVMCodeGenLevelAggressive,
        }
    }
    pub(super) fn llvm_pass_pipeline(&self) -> &'static str {
        match self {
            Self::O0 => "default<O0>",
            Self::O1 => "default<O1>",
            Self::O2 => "default<O2>",
            Self::O3 => "default<O3>",
        }
    }

    pub(crate) fn cc_opt_flag(&self) -> &'static str {
        match self {
            Self::O0 => "-O0",
            Self::O1 => "-O1",
            Self::O2 => "-O2",
            Self::O3 => "-O3",
        }
    }
}
