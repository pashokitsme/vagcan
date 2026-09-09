#!/usr/bin/env python3
"""disfn.py -- robust AArch64 function dumper for VCDS-arm64-unpacked.exe.

Decodes word-by-word (never stalls on data), annotates bl/adrp targets, and
stops at a plausible function end (ret followed by nothing, or the next .pdata
function boundary). Research tooling; clean-room interop.

usage: disfn.py 0x14006d900 [count]     # count words, default 160
"""
import sys
import pefile
from capstone import Cs, CS_ARCH_ARM64, CS_MODE_LITTLE_ENDIAN

EXE = "bin/VCDS-arm64-unpacked.exe"
IMAGE_BASE = 0x140000000
pe = pefile.PE(EXE)
with open(EXE, "rb") as f:
    DATA = f.read()
md = Cs(CS_ARCH_ARM64, CS_MODE_LITTLE_ENDIAN)


def off_of(vma):
    rva = vma - IMAGE_BASE
    for s in pe.sections:
        if s.VirtualAddress <= rva < s.VirtualAddress + max(s.Misc_VirtualSize, s.SizeOfRawData):
            return s.PointerToRawData + (rva - s.VirtualAddress)
    return None


def dump(vma, n):
    off = off_of(vma)
    adrp = {}
    for k in range(n):
        b = DATA[off + k * 4: off + k * 4 + 4]
        ins = next(md.disasm(b, vma + k * 4), None)
        if ins is None:
            print(f"  {vma + k*4:#010x}: .word {int.from_bytes(b,'little'):#010x}")
            continue
        note = ""
        m, ops = ins.mnemonic, ins.op_str
        if m == "adrp":
            try:
                rd, imm = ops.split(", ")
                adrp[rd] = int(imm.lstrip("#"), 0)
            except ValueError:
                pass
        elif m == "add":
            p = [x.strip() for x in ops.split(",")]
            if len(p) == 3 and p[0] == p[1] and p[2].startswith("#") and p[0] in adrp:
                note = f"  ; = {adrp[p[0]] + int(p[2][1:],0):#x}"
        elif m in ("ldr", "ldrb", "ldrh", "str") and "[" in ops:
            inside = ops[ops.find("[") + 1:ops.find("]")]
            pp = [x.strip() for x in inside.split(",")]
            if pp[0] in adrp:
                imm = int(pp[1].lstrip("#"), 0) if len(pp) > 1 and "#" in pp[1] else 0
                note = f"  ; [{adrp[pp[0]] + imm:#x}]"
        elif m in ("bl", "b") and ops.lstrip("#").startswith("0x"):
            note = f"  ; -> {int(ops.lstrip('#'),0):#x}"
        print(f"  {ins.address:#010x}: {m:<7} {ops}{note}")


if __name__ == "__main__":
    vma = int(sys.argv[1], 16)
    n = int(sys.argv[2]) if len(sys.argv) > 2 else 160
    dump(vma, n)
