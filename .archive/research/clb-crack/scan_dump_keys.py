#!/usr/bin/env python3
"""scan_dump_keys.py -- minidump-aware AES key-schedule scanner.

Walks a Windows minidump's Memory64List, scans each memory segment for AES
key schedules (reusing aes_ks_scan.expand), and reports every recovered master
key WITH the virtual address it lives at plus which loaded module owns that VA.
This gives both the key AND its location (which region / struct) in one pass,
and is far faster than raw-scanning the whole .dmp because we can restrict to
writable (heap/data) segments where live crypto contexts sit.

Usage:
    scan_dump_keys.py <dump.dmp>                # AES-256, writable regions
    scan_dump_keys.py <dump.dmp> --all-prot     # all committed regions
    scan_dump_keys.py <dump.dmp> --sizes 16,24,32
"""
import sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from aes_ks_scan import expand, _scan_one_layout, _wordswap
from minidump.minidumpfile import MinidumpFile


def module_for(va, modules):
    for m in modules:
        if m.baseaddress <= va < m.baseaddress + m.size:
            return os.path.basename(m.name.replace("\\", "/"))
    return "?"


WRITABLE_PROT = {"PAGE_READWRITE", "PAGE_WRITECOPY",
                 "PAGE_EXECUTE_READWRITE", "PAGE_EXECUTE_WRITECOPY"}


def build_prot_map(mf):
    """Return sorted list of (base, end, protect_name) for committed regions."""
    out = []
    if not mf.memory_info:
        return out
    for r in mf.memory_info.infos:
        name = r.Protect.name if r.Protect is not None else "?"
        out.append((r.BaseAddress, r.BaseAddress + r.RegionSize, name))
    out.sort()
    return out


def prot_writable(pmap, va):
    import bisect
    if not pmap:
        return True
    i = bisect.bisect_right([b for b, _, _ in pmap], va) - 1
    if i < 0:
        return False
    base, end, name = pmap[i]
    return base <= va < end and name in WRITABLE_PROT


def main():
    path = sys.argv[1]
    sizes = [32]
    if "--sizes" in sys.argv:
        sizes = [int(x) for x in sys.argv[sys.argv.index("--sizes") + 1].split(",")]
    all_prot = "--all-prot" in sys.argv

    mf = MinidumpFile.parse(path)
    modules = list(mf.modules.modules)
    reader = mf.get_reader()
    segs = mf.memory_segments_64.memory_segments
    pmap = build_prot_map(mf)

    print(f"# {path}: {len(segs)} segments", file=sys.stderr)
    seen = {}
    scanned = 0
    for s in segs:
        va = s.start_virtual_address
        size = s.size
        if not all_prot and not prot_writable(pmap, va):
            continue
        try:
            data = reader.read(va, size)
        except Exception:
            continue
        if not data:
            continue
        scanned += len(data)
        for layout_data, tag in ((data, ""), (_wordswap(data), " (wordswap)")):
            for off, keylen, khex in _scan_one_layout(layout_data, sizes, tag):
                kva = va + off
                mod = module_for(kva, modules)
                key = khex.split()[0]
                seen.setdefault(key, []).append((kva, keylen, mod, tag.strip()))
    print(f"# scanned {scanned/1e6:.1f} MB of {'all' if all_prot else 'writable'} regions",
          file=sys.stderr)
    print(f"# {len(seen)} distinct AES key(s):")
    for key, locs in seen.items():
        kl = locs[0][1]
        print(f"K(AES-{kl*8}) = {key}")
        for kva, keylen, mod, tag in locs:
            print(f"    @ VA {kva:#012x}  [{mod}] {tag}")
    if not seen:
        print("  (none)")


if __name__ == "__main__":
    main()
