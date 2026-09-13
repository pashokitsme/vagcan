import pefile,sys
EXE="bin/VCDS-arm64-unpacked.exe"; IB=0x140000000
pe=pefile.PE(EXE); DATA=open(EXE,'rb').read()
def off(vma):
    rva=vma-IB
    for s in pe.sections:
        if s.VirtualAddress<=rva<s.VirtualAddress+max(s.Misc_VirtualSize,s.SizeOfRawData):
            return s.PointerToRawData+(rva-s.VirtualAddress)
base=int(sys.argv[1],0); n=int(sys.argv[2]) if len(sys.argv)>2 else 32
o=off(base)
for i in range(n):
    p=int.from_bytes(DATA[o+i*8:o+i*8+8],'little')
    print(f"+{i*8:#04x} [{base+i*8:#x}] -> {p:#x}")
