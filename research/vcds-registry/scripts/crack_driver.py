"""Open, variant by variant in priority order, every file needed for the variant's full MWB list.
Runs `vagcan dev vcds rod <file> --cache <scratch ivcache> --dump dump/<stem>` (the shipped cracker);
stops when the CPU budget (seconds, children) is spent. Progress -> out/variant_sources.json after each variant."""
import os, sys, json, subprocess, resource, time
sys.path.insert(0, os.path.dirname(__file__))
import rodlib, inc
from alphabet import decoder
S = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
B = os.environ.get("VAGCAN") or os.path.join(os.path.dirname(os.path.abspath(__file__)), "../../../target/release/vagcan")
UDS = inc.UDS
budget = float(sys.argv[1])
plan = json.load(open(os.path.join(S, "out/plan.json")))
outp = os.path.join(S, "out/variant_sources.json")
done = json.load(open(outp)) if os.path.exists(outp) else {}
log = open(os.path.join(S, "logs/crack_driver.log"), "a")
def cpu(): r = resource.getrusage(resource.RUSAGE_CHILDREN); return r.ru_utime + r.ru_stime
cpu0 = cpu()
def secs(f): return {t: rodlib.classify(t, c, p, ci)[0] for t, c, p, ci in rodlib.sections(open(UDS + f, "rb").read())}
opened = {}
def open_file(f):
    """dump every section we can; returns (sections-kinds, dumpdir, cpu used)."""
    if f in opened: return opened[f]
    stem = f[:-4]; dd = os.path.join(S, "dump", stem); os.makedirs(dd, exist_ok=True)
    k = secs(f)
    need = any(v == "classic" for v in k.values())
    have = all(os.path.exists(os.path.join(dd, t + ".bin")) for t, v in k.items() if (t in ("INC", "MWB") and v == "classic") or v == "tea")
    c = 0.0
    if not have:
        t0 = cpu(); w0 = time.time()
        crack = k.get("INC") == "classic" or k.get("MWB") == "classic"
        cmd = [B, "dev", "vcds", "rod", UDS + f, "--cache", os.path.join(S, "ivcache.json"), "--dump", dd] + ([] if crack else ["--no-crack"])
        r = subprocess.run(cmd, capture_output=True, text=True)
        c = cpu() - t0
        log.write(f"{time.strftime('%H:%M:%S')} {f} cpu={c:.1f}s wall={time.time()-w0:.1f}s\n{r.stdout[-800:]}\n{r.stderr[-300:]}\n"); log.flush()
    opened[f] = (k, dd, c)
    return opened[f]
def inc_names(f, k, dd):
    if k.get("INC") == "classic":
        p = os.path.join(dd, "INC.bin")
        if not os.path.exists(p): return None
        txt = open(p, "rb").read().decode("latin-1")
        return [decoder(int(r[:6]))(r[7:]) for r in txt.split("\r\n") if len(r) > 7 and r[:6].isdigit()]
    if k.get("INC") == "tea":
        n, _ = inc.read_inc(f); return n
    return [] if "INC" not in k else None
def resolve(v, f, depth=0, seen=None):
    seen = seen or set()
    if f in seen or depth > 3: return [], []
    seen.add(f)
    k, dd, _ = open_file(f)
    srcs, blocks = [], []
    if "MWB" in k:
        if k["MWB"] == "shifted": blocks.append(("MWB shifted", f))
        elif os.path.exists(os.path.join(dd, "MWB.bin")): srcs.append(os.path.join(dd, "MWB.bin"))
        else: blocks.append(("MWB not opened", f))
    names = inc_names(f, k, dd)
    if names is None:
        blocks.append(("INC " + k.get("INC", "?"), f))
    else:
        for n in names:
            if n.startswith(("UNRESOLVED", "AMBIGUOUS")) or not os.path.exists(UDS + n + ".rod"):
                blocks.append(("INC entry unresolved", n)); continue
            kk = secs(n + ".rod")
            if "MWB" in kk or "INC" in kk:
                s2, b2 = resolve(v, n + ".rod", depth + 1, seen); srcs += s2; blocks += b2
    return srcs, blocks
order = [l.strip() for l in open(sys.argv[2]) if l.strip()]
for v in order:
    if v in done: continue
    if cpu() - cpu0 > budget: print("budget spent"); break
    f = plan[v]["file"]
    srcs, blocks = resolve(v, f)
    done[v] = {"file": f, "mwb": srcs, "blocked": blocks, "cpu_total": round(cpu() - cpu0, 1)}
    json.dump(done, open(outp, "w"), indent=1)
    print(f"{v:40s} {f:45s} mwb={len(srcs)} blocked={blocks} cpu={cpu()-cpu0:.0f}s", flush=True)
print("CPU used this run:", round(cpu() - cpu0, 1))
