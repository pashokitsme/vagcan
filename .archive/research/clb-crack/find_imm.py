import pefile
from capstone import Cs, CS_ARCH_ARM64, CS_MODE_LITTLE_ENDIAN
EXE="bin/VCDS-arm64-unpacked.exe"; IB=0x140000000
pe=pefile.PE(EXE); DATA=open(EXE,'rb').read(); md=Cs(CS_ARCH_ARM64,CS_MODE_LITTLE_ENDIAN)
text=[s for s in pe.sections if s.Name.rstrip(b'\x00')==b'.text'][0]
tva=IB+text.VirtualAddress; toff=text.PointerToRawData; n=text.Misc_VirtualSize//4
import sys
targets=[int(x,0) for x in sys.argv[1:]]
for k in range(n):
    vma=tva+k*4; b=DATA[toff+k*4:toff+k*4+4]
    ins=next(md.disasm(b,vma),None)
    if not ins: continue
    if ins.mnemonic in ("mov","movz") and ins.op_str.count("#"):
        try: imm=int(ins.op_str.split("#")[-1],0)
        except: continue
        if imm in targets and ins.op_str.split(",")[0].strip().startswith(("w","x")):
            print(f"{vma:#x}: {ins.mnemonic} {ins.op_str}")
