import pefile
import struct
import sys

def analyze(binary_path, target_addr):
    pe = pefile.PE(binary_path)
    image_base = pe.OPTIONAL_HEADER.ImageBase
    target_offset = target_addr - image_base
    
    print(f"Scanning for offset {hex(target_offset)} in data sections")
    
    # Pack as 32-bit and 64-bit little endian
    target_32 = struct.pack('<I', target_offset)
    target_64 = struct.pack('<Q', target_offset)
    
    for section in pe.sections:
        if not section.Name.startswith(b'.text'):
            data = section.get_data()
            idx = 0
            while True:
                idx = data.find(target_32, idx)
                if idx == -1: break
                ptr_addr = image_base + section.VirtualAddress + idx
                print(f"Found 32-bit offset in {section.Name.decode().strip(chr(0))} at {hex(ptr_addr)}")
                idx += 4
                
            idx = 0
            while True:
                idx = data.find(target_64, idx)
                if idx == -1: break
                ptr_addr = image_base + section.VirtualAddress + idx
                print(f"Found 64-bit offset in {section.Name.decode().strip(chr(0))} at {hex(ptr_addr)}")
                idx += 8

if __name__ == '__main__':
    analyze(sys.argv[1], int(sys.argv[2], 16))
