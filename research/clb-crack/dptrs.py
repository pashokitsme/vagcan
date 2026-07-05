#!/usr/bin/env python3
"""dptrs.py -- extract data pointers materialized inside a function.

Walks a function's words, tracks adrp per-reg, and for each add/ldr that
completes a page-relative address into .rdata/.data, reports the target VMA,
the instruction address, and a hexdump+entropy of the bytes there (so a KDF
secret loaded by the function is obvious).

usage: dptrs.py 0x14006d6c8 [nwords]
"""
import sys
import math
import pefile
from capstone import Cs, CS_ARCH_ARM64, CS_MODE_LITTLE_ENDIAN

EXE = "bin/VCDS-arm64-unpacked.exe"
IMAGE_BASE = 0x140000000
pe = pefile.PE(EXE)
with open(EXE, "rb") as f:
    DATA = f.read()
md = Cs(CS_ARCH_ARM64, CS_MODE_LITTLE_ENDIAN)


def sect_of(vma):
    rva = vma - IMAGE_BASE
    for s in pe.sections:
        if s.VirtualAddress <= rva < s.VirtualAddress + max(s.Misc_VirtualSize, s.SizeOfRawData):
            return s
    return None


def off_of(vma):
    s = sect_of(vma)
    if not s:
        return None
    return s.PointerToRawData + (vma - IMAGE_BASE - s.VirtualAddress)


def read(vma, n):
    off = off_of(vma)
    if off is None:
        return None
    return DATA[off:off + n]


def shannon(b):
    counts = [0] * 256
    for x in b:
        counts[x] += 1
    e = 0.0
    for c in counts:
        if c:
            p = c / len(b)
            e -= p * math.log2(p)
    return e


def dump(vma, n):
    off = off_of(vma)
    adrp = {}
    seen = set()
    for k in range(n):
        b = DATA[off + k * 4: off + k * 4 + 4]
        ins = next(md.disasm(b, vma + k * 4), None)
        if ins is None:
            continue
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
                tgt = adrp[p[0]] + int(p[2][1:], 0)
                report(ins.address, "add", tgt, seen)
        elif m in ("ldr", "ldrb", "ldrh") and "[" in ops:
            inside = ops[ops.find("[") + 1:ops.find("]")]
            pp = [x.strip() for x in inside.split(",")]
            if pp[0] in adrp:
                imm = int(pp[1].lstrip("#"), 0) if len(pp) > 1 and "#" in pp[1] else 0
                report(ins.address, "ldr", adrp[pp[0]] + imm, seen)


def report(at, kind, tgt, seen):
    s = sect_of(tgt)
    if not s:
        return
    nm = s.Name.decode().strip("\x00")
    if nm not in (".rdata", ".data"):
        return
    if tgt in seen:
        return
    seen.add(tgt)
    blob = read(tgt, 32)
    if blob is None:
        return
    ent = shannon(blob)
    txt = "".join(chr(x) if 0x20 <= x < 0x7f else "." for x in blob)
    print(f"  {at:#010x} {kind:3} -> {tgt:#012x} [{nm}] ent(32)={ent:.2f}  {blob.hex()}  |{txt}|")


if __name__ == "__main__":
    vma = int(sys.argv[1], 16)
    n = int(sys.argv[2]) if len(sys.argv) > 2 else 200
    print(f"# data pointers in fn {vma:#x} (n={n})")
    dump(vma, n)
