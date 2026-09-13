#!/usr/bin/env python3
"""For each bl to the send chokepoint, scan backwards to find w1(cmd), w3(len),
x2(data) setup. Heuristic: track last writes to w1/w3/x2 regs in a window."""
import sys
import pefile
from capstone import Cs, CS_ARCH_ARM64, CS_MODE_LITTLE_ENDIAN

EXE = "bin/VCDS-arm64-unpacked.exe"
IMAGE_BASE = 0x140000000
pe = pefile.PE(EXE)
DATA = open(EXE, "rb").read()
md = Cs(CS_ARCH_ARM64, CS_MODE_LITTLE_ENDIAN)
TARGET = 0x14006b640

def off_of(vma):
    rva = vma - IMAGE_BASE
    for s in pe.sections:
        if s.VirtualAddress <= rva < s.VirtualAddress + max(s.Misc_VirtualSize, s.SizeOfRawData):
            return s.PointerToRawData + (rva - s.VirtualAddress)
    return None

# find all text section bounds
text = None
for s in pe.sections:
    if s.Name.rstrip(b'\x00') == b'.text':
        text = s
tva0 = IMAGE_BASE + text.VirtualAddress
toff = text.PointerToRawData
tsize = text.Misc_VirtualSize

# scan all words for bl TARGET
callers = []
for k in range(tsize//4):
    vma = tva0 + k*4
    b = DATA[toff + k*4: toff + k*4 + 4]
    ins = next(md.disasm(b, vma), None)
    if ins and ins.mnemonic == "bl" and ins.op_str.lstrip("#").startswith("0x"):
        if int(ins.op_str.lstrip("#"),0) == TARGET:
            callers.append(vma)

def scan_back(call_vma, window=40):
    """decode window instrs before call, report last-set of w1,w3,x2,w2,x0"""
    off = off_of(call_vma)
    start = off - window*4
    regs = {}  # reg -> (vma, text)
    for k in range(window):
        a = call_vma - (window-k)*4
        b = DATA[start + k*4: start + k*4 + 4]
        ins = next(md.disasm(b, a), None)
        if not ins: continue
        m, ops = ins.mnemonic, ins.op_str
        # stop at previous bl (crossing call boundary muddies but keep going a bit)
        parts = [p.strip() for p in ops.split(",")]
        if parts:
            dst = parts[0]
            # record writes to interesting dests
            for tgt in ("w1","x1","w3","x3","w2","x2","w0","x0"):
                if dst == tgt and m in ("mov","movz","movn","orr","add","sub","ldr","ldrb","ldrh","adrp","and","mvn"):
                    regs[tgt] = (a, f"{m} {ops}")
    return regs

for c in callers:
    r = scan_back(c)
    w1 = r.get("w1") or r.get("x1")
    w3 = r.get("w3") or r.get("x3")
    print(f"\ncall @ {c:#x}")
    print(f"   w1(cmd) : {w1}")
    print(f"   w3(len) : {w3}")
    print(f"   x2(data): {r.get('x2') or r.get('w2')}")
