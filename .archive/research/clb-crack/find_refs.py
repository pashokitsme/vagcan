import struct
import sys

def find_refs(filename, target_addr, image_base=0x140000000):
    with open(filename, 'rb') as f:
        data = f.read()
    
    target_offset = target_addr - image_base
    print(f"Target offset: {hex(target_offset)}")
    
    # Search for direct 64-bit pointers
    ptr_bytes = struct.pack('<Q', target_addr)
    idx = 0
    while True:
        idx = data.find(ptr_bytes, idx)
        if idx == -1:
            break
        print(f"Found pointer at file offset: {hex(idx)} (RVA: {hex(idx)})")
        idx += 1

    # Search for ADRP / ADD pairs in AArch64
    # adrp xd, addr
    # add xd, xd, offset
    
    # We will just do a simple scan for ADRP that targets the page of target_addr
    target_page = target_addr & ~0xFFF
    
    for i in range(0, len(data) - 4, 4):
        inst = struct.unpack('<I', data[i:i+4])[0]
        # ADRP: op=1, 0, x, x (bit 31=1, 30-29=immlo, 28-24=10000, 23-5=immhi, 4-0=Rd)
        if (inst & 0x9F000000) == 0x90000000:
            rd = inst & 0x1F
            immhi = (inst >> 5) & 0x7FFFF
            immlo = (inst >> 29) & 3
            imm = (immhi << 2) | immlo
            # sign extend 21 bits
            if imm & 0x100000:
                imm -= 0x200000
            
            # calculate PC
            # assuming file offset == RVA for simplicity (need to parse PE sections ideally, but let's just approximate)
            # PE sections might have different file offset vs RVA. We should use pefile.
            pass

if __name__ == '__main__':
    find_refs(sys.argv[1], 0x140072ec0)
