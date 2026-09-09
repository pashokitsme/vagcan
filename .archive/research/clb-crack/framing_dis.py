#!/usr/bin/env python3
"""framing_dis.py -- dump AArch64 disasm of the CB-packet framing functions in
VCDS-arm64-unpacked.exe, for reversing the cable wire format (vag-hex interop).

Pure research tooling. Does NOT touch crates/.
"""
import sys
import pefile
from capstone import Cs, CS_ARCH_ARM64, CS_MODE_LITTLE_ENDIAN

EXE = "bin/VCDS-arm64-unpacked.exe"
IMAGE_BASE = 0x140000000

pe = pefile.PE(EXE, fast_load=True)

def vma_to_off(vma):
    rva = vma - IMAGE_BASE
    for s in pe.sections:
        start = s.VirtualAddress
        end = start + max(s.Misc_VirtualSize, s.SizeOfRawData)
        if start <= rva < end:
            return s.PointerToRawData + (rva - start)
    return None

with open(EXE, "rb") as f:
    data = f.read()

md = Cs(CS_ARCH_ARM64, CS_MODE_LITTLE_ENDIAN)
md.detail = True

def dump(vma, n_ins, label):
    print(f"\n===== {label}  @ {vma:#x} =====")
    off = vma_to_off(vma)
    if off is None:
        print("  (vma not in any section)")
        return
    code = data[off: off + n_ins * 4]
    addr = vma
    i = 0
    for ins in md.disasm(code, vma):
        # detect function boundary: a lone 'ret' after some body
        print(f"  {ins.address:#010x}: {ins.mnemonic:<8} {ins.op_str}")
        i += 1
        if ins.mnemonic == "ret" and i > 4:
            print("  --- ret (fn end) ---")
            break

if __name__ == "__main__":
    targets = [
        (0x14006b640, 200, "SEND / encode chokepoint"),
        (0x14006bb04, 200, "RECV / unwrap (Pulling packet from buffer)"),
        (0x14006cf90, 200, "CB send + encrypt fork"),
        (0x14006d3b8, 160, "encrypt routine"),
        (0x14006dc5c, 160, "recv"),
    ]
    # allow overriding via CLI: framing_dis.py 0x... [count]
    if len(sys.argv) >= 2:
        vma = int(sys.argv[1], 16)
        n = int(sys.argv[2]) if len(sys.argv) >= 3 else 200
        dump(vma, n, "custom")
    else:
        for vma, n, label in targets:
            dump(vma, n, label)
