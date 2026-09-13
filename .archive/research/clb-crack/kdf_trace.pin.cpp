// kdf_trace.pin.cpp -- Tier B pintool: trace the VMProtect VM while VCDS derives K_epoch.
//
// Follows the hackyboiz Pin+Triton devirt pipeline. Emits 3 artifacts the Triton
// harness (kdf_triton.py) consumes:
//   vmtrace.out            -- every executed insn (addr + machine bytes) inside the VM
//   bytecode_values.txt    -- the operand word read from the VM bytecode ptr at each fetch
//   handler_registers.txt  -- register snapshot at each handler entry
//
// TARGET:  OLD x86 (ia32) VCDS.exe on a native x86-64 Windows host.
// BUILD:   REQUIRES the Intel Pin SDK -- this is NOT part of the Rust workspace build
//          and will NOT compile in this repo (pin.H / ADDRINT resolve only against the
//          Pin kit; local clang "file not found" errors are EXPECTED). Copy into a Pin
//          sample dir and build there:  make PIN_ROOT=/path/to/pin TARGET=ia32
// RUN:     pin -t kdf_trace.dll -- VCDS.exe
//          then drive ONE engine-01 open in VCDS so exactly one KDF runs, then quit.
//
// ---- TWO THINGS YOU MUST FILL IN (from static analysis of the unpacked image) ----
//   1) VM_ENTRY_RVA   : RVA of the push/call into the VM interpreter for the KDF
//                       (the code between the b6 frame out and the first KS encrypt).
//   2) VMP_SECTION    : name/range of the .vmp section (Detect It Easy reports it),
//                       so we only instrument inside the VM and keep the trace small.
// The VM bytecode pointer register (ESI in the hackyboiz sample) may differ; adjust
// BYTECODE_PTR_REG and confirm from the trace (the reg that advances by opcode+operand).

#include "pin.H"
#include <fstream>
#include <iostream>

// ------- fill these from static RE (see PATH2 doc S B.1 / B.0) --------------------
static ADDRINT VM_ENTRY_RVA   = 0x00000000; // TODO: RVA of the KDF VM entry
static ADDRINT VMP_SEC_LO_RVA = 0x00000000; // TODO: .vmp section start RVA
static ADDRINT VMP_SEC_HI_RVA = 0x00000000; // TODO: .vmp section end   RVA
#define BYTECODE_PTR_REG REG_ESI            // hackyboiz VM used ESI; verify from trace
// ---------------------------------------------------------------------------------

static std::ofstream fTrace, fBytecode, fRegs;
static ADDRINT g_imgLow = 0, g_imgHigh = 0, g_imgBase = 0;
static ADDRINT g_vmEntry = 0, g_vmLo = 0, g_vmHi = 0;
static bool    g_inVM = false;

KNOB<std::string> KnobMain(KNOB_MODE_WRITEONCE, "pintool", "img", "VCDS.exe",
                           "main image name to anchor RVAs on");

static bool inVmSection(ADDRINT a) { return a >= g_vmLo && a < g_vmHi; }

// per-instruction analysis: log while executing inside the VM region
VOID OnInsn(ADDRINT ip, UINT32 size, const CONTEXT *ctx) {
    if (ip == g_vmEntry) g_inVM = true;          // entered the KDF VM
    if (!g_inVM) return;
    if (!inVmSection(ip)) { g_inVM = false; return; } // left the VM -> stop logging

    // 1) instruction trace: address + machine bytes
    UINT8 buf[16] = {0};
    PIN_SafeCopy(buf, (VOID *)ip, size > 16 ? 16 : size);
    fTrace << std::hex << ip << " ";
    for (UINT32 i = 0; i < size && i < 16; i++)
        fTrace << (buf[i] < 16 ? "0" : "") << (unsigned)buf[i];
    fTrace << "\n";
}

// at a handler entry (dispatcher just fetched an opcode): dump the bytecode operand
// word and a register snapshot. We approximate "handler entry" as any insn that reads
// through the bytecode pointer register; refine to the exact dispatcher fetch addr once
// known from the frequency analysis (uniq -c vmtrace.out).
VOID OnHandlerEntry(ADDRINT ip, ADDRINT bcptr, const CONTEXT *ctx) {
    if (!g_inVM) return;
    UINT32 word = 0;
    PIN_SafeCopy(&word, (VOID *)bcptr, sizeof(word));
    fBytecode << std::hex << ip << " bc=" << bcptr << " word=" << word << "\n";

    fRegs << std::hex << "ip=" << ip
          << " eax=" << PIN_GetContextReg(ctx, REG_EAX)
          << " ebx=" << PIN_GetContextReg(ctx, REG_EBX)
          << " ecx=" << PIN_GetContextReg(ctx, REG_ECX)
          << " edx=" << PIN_GetContextReg(ctx, REG_EDX)
          << " esi=" << PIN_GetContextReg(ctx, REG_ESI)
          << " edi=" << PIN_GetContextReg(ctx, REG_EDI)
          << " ebp=" << PIN_GetContextReg(ctx, REG_EBP)
          << " esp=" << PIN_GetContextReg(ctx, REG_ESP)
          << " eflags=" << PIN_GetContextReg(ctx, REG_GFLAGS) << "\n";
}

VOID Instruction(INS ins, VOID *) {
    ADDRINT ip = INS_Address(ins);
    if (ip < g_imgLow || ip >= g_imgHigh) return; // only our image

    INS_InsertCall(ins, IPOINT_BEFORE, (AFUNPTR)OnInsn,
                   IARG_INST_PTR, IARG_UINT32, INS_Size(ins),
                   IARG_CONST_CONTEXT, IARG_END);

    // heuristic handler-entry hook: instructions that read via the bytecode ptr reg.
    // Once the dispatcher fetch address is known, replace this with an exact addr test.
    if (INS_IsMemoryRead(ins) && INS_RegRContain(ins, BYTECODE_PTR_REG)) {
        INS_InsertCall(ins, IPOINT_BEFORE, (AFUNPTR)OnHandlerEntry,
                       IARG_INST_PTR, IARG_REG_VALUE, BYTECODE_PTR_REG,
                       IARG_CONST_CONTEXT, IARG_END);
    }
}

VOID ImageLoad(IMG img, VOID *) {
    std::string nm = IMG_Name(img);
    if (nm.find(KnobMain.Value()) == std::string::npos) return;
    g_imgBase = IMG_LoadOffset(img);
    g_imgLow  = IMG_LowAddress(img);
    g_imgHigh = IMG_HighAddress(img);
    g_vmEntry = g_imgLow + VM_ENTRY_RVA;       // rebase RVA -> live VA
    g_vmLo    = g_imgLow + VMP_SEC_LO_RVA;
    g_vmHi    = g_imgLow + VMP_SEC_HI_RVA;
    std::cerr << "[kdf_trace] img=" << nm << " low=" << std::hex << g_imgLow
              << " vmentry=" << g_vmEntry << " vm=[" << g_vmLo << "," << g_vmHi << ")\n";
}

VOID Fini(INT32, VOID *) { fTrace.close(); fBytecode.close(); fRegs.close(); }

int main(int argc, char *argv[]) {
    if (PIN_Init(argc, argv)) { std::cerr << "PIN_Init failed\n"; return 1; }
    fTrace.open("vmtrace.out");
    fBytecode.open("bytecode_values.txt");
    fRegs.open("handler_registers.txt");
    IMG_AddInstrumentFunction(ImageLoad, 0);
    INS_AddInstrumentFunction(Instruction, 0);
    PIN_AddFiniFunction(Fini, 0);
    PIN_StartProgram();
    return 0;
}
