#!/usr/bin/env python3
"""xref.py -- find references to a target VMA in VCDS-arm64-unpacked.exe.

Three reference kinds are located:
  1. code:   adrp(+add|+ldr) pairs that materialize the target address into a reg
  2. data:   the 8-byte little-endian pointer value stored in a data section
  3. reloc:  base-relocation entries whose slot holds the target pointer

Pure research tooling (clean-room interop for the owner's own cable). No crates/.

usage: xref.py 0x140072ec0
"""
import sys
import pefile
from capstone import Cs, CS_ARCH_ARM64, CS_MODE_LITTLE_ENDIAN

EXE = "bin/VCDS-arm64-unpacked.exe"
IMAGE_BASE = 0x140000000

pe = pefile.PE(EXE)
with open(EXE, "rb") as f:
    DATA = f.read()


def sect_of(rva):
    for s in pe.sections:
        start = s.VirtualAddress
        end = start + max(s.Misc_VirtualSize, s.SizeOfRawData)
        if start <= rva < end:
            return s
    return None


def off_of(vma):
    s = sect_of(vma - IMAGE_BASE)
    if not s:
        return None
    return s.PointerToRawData + (vma - IMAGE_BASE - s.VirtualAddress)


def code_xrefs(target):
    """adrp reg,page ; add/ldr reg,reg,#lo -> reg == target"""
    md = Cs(CS_ARCH_ARM64, CS_MODE_LITTLE_ENDIAN)
    hits = []
    for s in pe.sections:
        if not (s.Characteristics & 0x20000000):  # IMAGE_SCN_MEM_EXECUTE
            continue
        base = IMAGE_BASE + s.VirtualAddress
        code = s.get_data()
        # track last adrp result per register
        adrp = {}  # reg -> (page_base, addr_of_adrp)
        for off in range(0, len(code) - 3, 4):
            gi = md.disasm(code[off:off + 4], base + off)
            ins = next(gi, None)
            if ins is None:
                continue
            m = ins.mnemonic
            if m in ("bl", "b") and ins.op_str.lstrip("#").startswith("0x"):
                if int(ins.op_str.lstrip("#"), 0) == target:
                    hits.append((m + " (direct call)", ins.address, ins.address, ""))
            if m == "adrp":
                try:
                    rd, imm = ins.op_str.split(", ")
                    adrp[rd] = (int(imm.lstrip("#"), 0), ins.address)
                except ValueError:
                    pass
            elif m == "add":
                parts = [p.strip() for p in ins.op_str.split(",")]
                if len(parts) == 3 and parts[0] == parts[1] and parts[2].startswith("#"):
                    rd = parts[0]
                    if rd in adrp:
                        val = adrp[rd][0] + int(parts[2][1:], 0)
                        if val == target:
                            hits.append(("adrp+add", adrp[rd][1], ins.address, rd))
            elif m == "ldr":
                # ldr rd, [rn, #imm]  -> loads *(page+imm); if page+imm==target slot
                if "[" in ins.op_str:
                    inside = ins.op_str[ins.op_str.find("[") + 1:ins.op_str.find("]")]
                    parts = [p.strip() for p in inside.split(",")]
                    rn = parts[0]
                    imm = 0
                    if len(parts) > 1 and parts[1].startswith("#"):
                        imm = int(parts[1][1:], 0)
                    if rn in adrp:
                        slot = adrp[rn][0] + imm
                        if slot == target:
                            hits.append(("adrp+ldr(slot)", adrp[rn][1], ins.address, rn))
    return hits


def data_xrefs(target):
    """8-byte LE pointer == target, stored anywhere."""
    needle = target.to_bytes(8, "little")
    hits = []
    for s in pe.sections:
        if s.Characteristics & 0x20000000:
            continue
        data = s.get_data()
        i = 0
        while True:
            i = data.find(needle, i)
            if i == -1:
                break
            vma = IMAGE_BASE + s.VirtualAddress + i
            hits.append((s.Name.decode().strip("\x00"), vma))
            i += 1
    return hits


if __name__ == "__main__":
    target = int(sys.argv[1], 16)
    print(f"target = {target:#x}\n")
    print("== code xrefs (adrp materialize) ==")
    for kind, a, b, reg in code_xrefs(target):
        print(f"  {kind:16} {a:#010x} .. {b:#010x}  ({reg})")
    print("\n== data xrefs (stored 8-byte pointer) ==")
    for name, vma in data_xrefs(target):
        print(f"  {name:10} slot @ {vma:#010x}")
