#include <llvm-c/Analysis.h>
#include <llvm-c/Core.h>
#include <llvm-c/Error.h>
#include <llvm-c/Target.h>
#include <llvm-c/TargetMachine.h>
#include <llvm-c/Transforms/PassBuilder.h>

void LLVMInitializeAllTargetInfosShim(void);
void LLVMInitializeAllTargetsShim(void);
void LLVMInitializeAllTargetMCsShim(void);
void LLVMInitializeAllAsmPrintersShim(void);
void LLVMInitializeAllAsmParsersShim(void);
