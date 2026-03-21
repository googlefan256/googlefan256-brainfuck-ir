#include <llvm-c/Target.h>

void LLVMInitializeAllTargetInfosShim(void) {
    LLVMInitializeAllTargetInfos();
}

void LLVMInitializeAllTargetsShim(void) {
    LLVMInitializeAllTargets();
}

void LLVMInitializeAllTargetMCsShim(void) {
    LLVMInitializeAllTargetMCs();
}

void LLVMInitializeAllAsmPrintersShim(void) {
    LLVMInitializeAllAsmPrinters();
}

void LLVMInitializeAllAsmParsersShim(void) {
    LLVMInitializeAllAsmParsers();
}
