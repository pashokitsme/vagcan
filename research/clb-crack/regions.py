#!/usr/bin/env python3
"""regions.py -- find contiguous high-entropy runs in .rdata/.data (block-level).

Splits each data section into 16-byte blocks, marks each as high-entropy if it
is not ascii, not mostly-zero, and byte-diverse; then merges adjacent hi blocks
into runs. Reports run [start,end) length. Isolated short runs (16-64 bytes)
are the interesting secret-candidates; long runs are tables/blobs.
"""
import pefile

EXE = "bin/VCDS-arm64-unpacked.exe"
IMAGE_BASE = 0x140000000
pe = pefile.PE(EXE)
with open(EXE, "rb") as f:
    DATA = f.read()

B = 16


def hi(block):
    if len(block) < B:
        return False
    z = block.count(0)
    ff = block.count(0xff)
    if z > 6 or ff > 6:
        return False
    printable = sum(1 for x in block if 0x20 <= x < 0x7f)
    if printable >= B * 0.85:
        return False
    return len(set(block)) >= 12


for s in pe.sections:
    nm = s.Name.decode().strip("\x00")
    if nm not in (".rdata", ".data"):
        continue
    raw = DATA[s.PointerToRawData:s.PointerToRawData + s.SizeOfRawData]
    base = IMAGE_BASE + s.VirtualAddress
    runs = []
    cur = None
    for i in range(0, len(raw) - B, B):
        if hi(raw[i:i + B]):
            if cur is None:
                cur = [i, i + B]
            else:
                cur[1] = i + B
        else:
            if cur is not None:
                runs.append(tuple(cur))
                cur = None
    if cur:
        runs.append(tuple(cur))
    print(f"== {nm} base={base:#x} rawlen={len(raw):#x} runs={len(runs)} ==")
    for a, b in runs:
        L = b - a
        tag = "  <-- SHORT/ISOLATED" if L <= 64 else ""
        print(f"  {base+a:#012x} .. {base+b:#012x}  len={L:#x} ({L}){tag}")
