#!/usr/bin/env python3
"""kdf_triton.py -- Tier B: lift VMProtect handler semantics with Triton.

Consumes the 3 artifacts emitted by kdf_trace.pin.cpp:
    vmtrace.out            -- executed insns (addr + machine bytes) inside the VM
    bytecode_values.txt    -- bytecode operand word at each fetch  (ip bc=.. word=..)
    handler_registers.txt  -- register snapshot at each handler entry

Pipeline (hackyboiz VMPpart2 method), each stage is a function below:
  1. frequency-rank vmtrace.out -> dispatcher (highest count) + handler addrs.
  2. segment the trace into per-handler blocks (glue + body).
  3. cluster identical-byte segments -> one canonical representative per handler.
  4. Triton-execute each canonical handler symbolically with its entry register
     snapshot; diff pre/post regs+stack -> name the handler (LCONST/ADD/XOR/ROL/
     load/store/...) and its bytecode-ptr advance.
  5. cross-ref dispatcher fetch (bytecode_values.txt) -> opcode -> handler -> semantics.
  6. emit the opcode->semantics table; the KDF is then read off the bytecode stream.

Then reimplement the recovered KDF in Rust (future crates/vag-hex/src/kdf.rs) and
validate every candidate K via the S0 oracle (validate_k.py / the KS_F3 check).

This is a HARNESS OUTLINE: the Triton context setup + arch is concrete; the trace
parsing is real; the per-handler semantic classifier has clear TODO hooks. Run on the
native x86 host where the Pin artifacts were produced.

    pip install triton-library capstone keystone-engine
    python kdf_triton.py vmtrace.out bytecode_values.txt handler_registers.txt
"""
import sys, re
from collections import Counter, defaultdict

try:
    from triton import TritonContext, ARCH, Instruction, MemoryAccess, CPUSIZE
except ImportError:
    TritonContext = None  # allow --parse-only without triton installed


# ---- stage 1: dispatcher + handler identification ---------------------------
def load_trace(path):
    """[(addr:int, bytes)] in execution order."""
    out = []
    for line in open(path):
        m = re.match(r"([0-9a-fA-F]+)\s+([0-9a-fA-F]+)", line)
        if m:
            out.append((int(m.group(1), 16), bytes.fromhex(m.group(2))))
    return out


def rank_addrs(trace):
    c = Counter(a for a, _ in trace)
    ranked = c.most_common()
    dispatcher = ranked[0][0] if ranked else None
    return dispatcher, ranked


# ---- stage 2: segment into per-handler blocks -------------------------------
def segment(trace, dispatcher):
    """Split the linear trace each time we return to the dispatcher -> one segment
    per VM instruction (dispatcher glue + the handler body it jumped to)."""
    segs, cur = [], []
    for a, b in trace:
        if a == dispatcher and cur:
            segs.append(cur)
            cur = []
        cur.append((a, b))
    if cur:
        segs.append(cur)
    return segs


# ---- stage 3: cluster identical segments ------------------------------------
def cluster(segs):
    """Group segments with identical (addr,bytes) sequences. Returns
    {signature: [seg_indices]} and a canonical representative per cluster."""
    groups = defaultdict(list)
    for i, s in enumerate(segs):
        sig = tuple((a, b) for a, b in s)
        groups[sig].append(i)
    canon = {sig: idxs[0] for sig, idxs in groups.items()}
    return groups, canon


# ---- register snapshots -----------------------------------------------------
def load_regs(path):
    """[dict] register snapshots at handler entries, in order."""
    snaps = []
    for line in open(path):
        d = {}
        for k, v in re.findall(r"(\w+)=([0-9a-fA-F]+)", line):
            d[k] = int(v, 16)
        if d:
            snaps.append(d)
    return snaps


def load_bytecode(path):
    """[(ip, bcptr, word)] fetched operands, in order."""
    out = []
    for line in open(path):
        m = re.match(r"([0-9a-fA-F]+)\s+bc=([0-9a-fA-F]+)\s+word=([0-9a-fA-F]+)", line)
        if m:
            out.append((int(m.group(1), 16), int(m.group(2), 16), int(m.group(3), 16)))
    return out


# ---- stage 4: Triton semantic lift of one canonical handler -----------------
def lift_handler(seg, entry_regs):
    """Symbolically execute a handler segment; return a semantic summary.

    Sets up an x86 (ia32) Triton context, seeds concrete register state from the
    Pin snapshot, steps every insn, then diffs post vs pre state:
      - which regs changed, by how much (constant delta => counter/ptr advance)
      - the bytecode-ptr advance (ESI += 5 in hackyboiz = 1B opcode + 4B operand)
      - arithmetic chain on the operand (add/neg/rol/not => the constant-decrypt)
    TODO: classify into {LCONST, ADD, SUB, XOR, AND, OR, ROL, LOAD, STORE, PUSH,
    POP, JMP, VMEXIT} from the diff signature; that mapping is the deliverable.
    """
    if TritonContext is None:
        return {"note": "triton not installed; parse-only"}
    ctx = TritonContext()
    ctx.setArchitecture(ARCH.X86)  # 32-bit
    # seed concrete registers from the Pin snapshot
    reg = ctx.registers
    name2reg = {
        "eax": reg.eax, "ebx": reg.ebx, "ecx": reg.ecx, "edx": reg.edx,
        "esi": reg.esi, "edi": reg.edi, "ebp": reg.ebp, "esp": reg.esp,
    }
    for n, r in name2reg.items():
        if n in entry_regs:
            ctx.setConcreteRegisterValue(r, entry_regs[n])
    pre = {n: ctx.getConcreteRegisterValue(r) for n, r in name2reg.items()}

    for addr, code in seg:
        insn = Instruction(addr, code)
        ctx.processing(insn)

    post = {n: ctx.getConcreteRegisterValue(r) for n, r in name2reg.items()}
    delta = {n: (post[n] - pre[n]) & 0xFFFFFFFF for n in pre}
    return {
        "esi_advance": delta.get("esi"),   # opcode+operand stride (e.g. 5)
        "changed": {n: (pre[n], post[n]) for n in pre if pre[n] != post[n]},
        # TODO: symbolic-expression readout of the operand transform for LCONST-style
        #       handlers (ctx.getSymbolicRegister(...).getAst() -> simplify).
        "semantic": "TODO-classify",
    }


# ---- stage 5/6: opcode -> handler -> semantics table ------------------------
def build_opcode_map(trace, dispatcher, segs, canon, regs, bytecode):
    """Cross-reference the dispatcher fetch (bytecode word) with each segment's
    handler target to produce {opcode_word: (handler_addr, semantics)}."""
    table = {}
    for i, seg in enumerate(segs):
        if len(seg) < 2:
            continue
        handler_addr = seg[1][0]  # first insn after the dispatcher glue
        word = bytecode[i][2] if i < len(bytecode) else None
        entry = regs[i] if i < len(regs) else {}
        sem = lift_handler(seg, entry)
        key = word if word is not None else handler_addr
        table[key] = (handler_addr, sem)
    return table


def main():
    tr = load_trace(sys.argv[1])
    bc = load_bytecode(sys.argv[2]) if len(sys.argv) > 2 else []
    rg = load_regs(sys.argv[3]) if len(sys.argv) > 3 else []

    dispatcher, ranked = rank_addrs(tr)
    print(f"# {len(tr)} insns; dispatcher (top freq) = {dispatcher:#x}")
    print("# top 10 addrs by frequency (dispatcher + handlers):")
    for a, n in ranked[:10]:
        print(f"    {a:#010x}  x{n}")

    segs = segment(tr, dispatcher)
    groups, canon = cluster(segs)
    print(f"# {len(segs)} segments -> {len(groups)} unique handler clusters")

    table = build_opcode_map(tr, dispatcher, segs, canon, rg, bc)
    print("# opcode -> (handler, semantics):")
    for k, (h, sem) in sorted(table.items(), key=lambda x: (isinstance(x[0], int), x[0])):
        kk = f"{k:#x}" if isinstance(k, int) else str(k)
        print(f"    op {kk:>12} -> handler {h:#010x}  esi+={sem.get('esi_advance')}"
              f"  {sem.get('semantic')}")

    print("\n# NEXT: name each handler from its diff signature, walk the .vmp bytecode")
    print("#       stream through this table to recover the KDF as pseudo-code, then")
    print("#       reimplement in Rust and gate every candidate K on validate_k.py (S0).")


if __name__ == "__main__":
    main()
