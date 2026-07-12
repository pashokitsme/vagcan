#!/usr/bin/env python3
"""Analysis for coolant-rpm-speed.pcapng + .CSV (new engine-running capture).

Stage 1: extract per-DID RDBI response time-series (TP-crib channels).
Stage 2: parse the ADVMB CSV -> per-measurement (ide,name,unit,[(t,v)]).
Stage 3: shape-align each DID x interpretation to each CSV measurement.
Stage 4: least-squares fit raw->value, report residuals + R^2.

Analysis-only. Dumps are gitignored; never committed.
"""
import sys, os
from collections import defaultdict, Counter
import numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from usbpcap import reassemble_frames

CAP = os.path.join(os.path.dirname(__file__), "../dumps/coolant-rpm-speed.pcapng")
CSV = os.path.join(os.path.dirname(__file__), "../dumps/coolant-rpm-speed.CSV")
TP = bytes([0x02, 0x3E, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00])
OKPCI = {0x02, 0x03, 0x04, 0x05, 0x06, 0x07}
OKSID = {0x3E, 0x22, 0x19, 0x2E}


def ck(b):
    return (b[0], b[2], b[3], b[5])


def tp_ks(reqs):
    for cand in [b for b, _ in Counter(reqs).most_common()]:
        ks = {6 + i: cand[6 + i] ^ TP[i] for i in range(8)}
        if all((b[6] ^ ks[6]) in OKPCI and (b[7] ^ ks[7]) in OKSID for b in set(reqs)):
            return ks
    return None


def load():
    b8, b7 = [], []
    for f in reassemble_frames(CAP):
        p = f["payload"]
        if not p or len(p) < 17:
            continue
        blk = bytes(p[1:17])
        if p[0] == 0xB8 and f["dir"] == "OUT":
            b8.append((f["t"], blk))
        elif p[0] == 0xB7 and f["dir"] == "IN":
            b7.append((f["t"], blk))
    return b8, b7


def extract():
    """Return {did: [(t, data_bytes_after_DID_echo)]} for single-frame RDBI."""
    b8, b7 = load()
    chreq, chrsp = defaultdict(list), defaultdict(list)
    for t, b in b8:
        chreq[ck(b)].append(b)
    for t, b in b7:
        chrsp[ck(b)].append((t, b))
    series = defaultdict(list)
    pci_hist = defaultdict(Counter)
    for c, reqs in chreq.items():
        ks = tp_ks(reqs)
        if ks is None:
            continue
        for t, b in chrsp.get(c, []):
            d = bytes(b[i] ^ ks.get(i, 0) for i in range(16))
            if d[7] != 0x62:
                continue
            did = (d[8] << 8) | d[9]
            pci = d[6]
            pci_hist[did][pci] += 1
            if pci & 0xF0 == 0x00:
                n = pci & 0x0F
                data = d[10:7 + n]
            else:
                data = d[10:14]
            series[did].append((t, bytes(data)))
    return series, pci_hist


def parse_csv():
    L = [l.rstrip("\n").rstrip("\r") for l in open(CSV, encoding="cp1251")]
    ide = L[4].split(",")
    name = L[5].split(",")
    unit = L[6].split(",")
    data = [d for d in L[7:] if d.strip(",") != ""]
    out = []
    for j, h in enumerate(ide):
        if "IDE" not in h:
            continue
        ser = []
        for d in data:
            c = d.split(",")
            if j < len(c) and c[j].strip() and c[j - 1].strip():
                try:
                    ser.append((float(c[j - 1]), float(c[j])))
                except ValueError:
                    pass
        if len(ser) < 5:
            continue
        u = unit[j].strip() if j < len(unit) else ""
        nm = name[j].strip() if j < len(name) else ""
        out.append({"id": h.replace("Loc.", "").strip(), "unit": u, "name": nm, "series": ser})
    return out


FORMS = {
    "u8[0]":  lambda d: d[0],
    "u8[1]":  lambda d: d[1] if len(d) > 1 else d[0],
    "u16be":  lambda d: ((d[0] << 8) | d[1]) if len(d) > 1 else d[0],
    "u16le":  lambda d: ((d[1] << 8) | d[0]) if len(d) > 1 else d[0],
    "i16be":  lambda d: ((((d[0] << 8 | d[1]) ^ 0x8000) - 0x8000) if len(d) > 1 else d[0]),
}


def align(rt, rv, csv, dt=0.4, maxlag=60):
    tr = np.asarray(rt, float); vr = np.asarray(rv, float)
    tc = np.asarray([t for t, _ in csv], float); vc = np.asarray([v for _, v in csv], float)
    tr = tr - tr[0]; tc = tc - tc[0]
    if vr.std() < 1e-9 or vc.std() < 1e-9:
        return None
    gr = np.arange(0, tc[-1], dt); ci = np.interp(gr, tc, vc)
    best = None
    for k in range(int(-maxlag / dt), int(maxlag / dt) + 1):
        ri = np.interp(gr, tr + k * dt, vr, left=np.nan, right=np.nan)
        m = ~np.isnan(ri)
        if m.sum() < 20 or ri[m].std() < 1e-9:
            continue
        r = np.corrcoef(ri[m], ci[m])[0, 1]
        if best is None or abs(r) > abs(best[0]):
            best = (r, k * dt, int(m.sum()))
    return best


def fit(rt, rv, csv, lag, dt=0.4):
    """Least-squares value = a*raw + b at the aligned lag. Returns (a,b,R2,n,pairs)."""
    tr = np.asarray(rt, float) - rt[0]
    vr = np.asarray(rv, float)
    tc = np.asarray([t for t, _ in csv], float); tc = tc - tc[0]
    vc = np.asarray([v for _, v in csv], float)
    gr = np.arange(0, tc[-1], dt)
    ci = np.interp(gr, tc, vc)
    ri = np.interp(gr, tr + lag, vr, left=np.nan, right=np.nan)
    m = ~np.isnan(ri)
    x = ri[m]; y = ci[m]
    if len(x) < 20 or x.std() < 1e-9:
        return None
    A = np.vstack([x, np.ones_like(x)]).T
    (a, b), *_ = np.linalg.lstsq(A, y, rcond=None)
    pred = a * x + b
    ss_res = ((y - pred) ** 2).sum()
    ss_tot = ((y - y.mean()) ** 2).sum()
    r2 = 1 - ss_res / ss_tot if ss_tot > 0 else 0
    resid = np.sqrt(ss_res / len(x))
    return a, b, r2, len(x), resid, x, y


def main():
    series, pci_hist = extract()
    csvs = parse_csv()
    print(f"# capture: {len(series)} DIDs ; CSV: {len(csvs)} measurements\n")

    print("# === DID raw series ===")
    for did in sorted(series):
        s = sorted(series[did])
        vals = Counter(v.hex(' ') for _, v in s)
        span = s[-1][0] - s[0][0]
        pcis = ' '.join(f"{p:02x}:{n}" for p, n in pci_hist[did].most_common())
        print(f"DID {did:04X}: n={len(s)} distinct={len(vals)} span={span:.0f}s PCI[{pcis}]")
        for hexv, n in vals.most_common(4):
            print(f"     {hexv:16s} x{n}")

    print("\n# === CSV measurements ===")
    for m in csvs:
        vs = [v for _, v in m["series"]]
        print(f"{m['id']} [{m['unit']}] n={len(m['series'])} "
              f"range=[{min(vs):.2f},{max(vs):.2f}]  {m['name'][:40]}")

    print("\n# === alignment: each DID x form vs each CSV measurement ===")
    res = []
    for did in sorted(series):
        s = sorted(series[did])
        rt = [t for t, _ in s]
        for fn, fv in FORMS.items():
            try:
                rv = [fv(v) for _, v in s]
            except Exception:
                continue
            if len(set(rv)) < 2:
                continue
            for m in csvs:
                b = align(rt, rv, m["series"])
                if b:
                    res.append((abs(b[0]), b[0], b[1], b[2], did, fn, m["id"], m["unit"]))
    res.sort(reverse=True)
    for ar, r, lag, n, did, fn, mid, u in res[:24]:
        print(f"  |r|={ar:.3f} r={r:+.3f} lag={lag:+5.1f} n={n:3d}  DID{did:04X} {fn:6s}"
              f"  {mid} [{u}]")


if __name__ == "__main__":
    main()
