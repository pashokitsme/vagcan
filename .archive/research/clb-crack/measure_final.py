#!/usr/bin/env python3
"""Unified extraction + alignment: recover EVERY RDBI DID data time-series from
capture-w-logs.pcapng (TP-crib channels with a full keystream, plus non-crib
channels via the request-padding two-time-pad), then cross-correlate each raw
interpretation against every ADVMB CSV measurement (both logs) with a free time
lag over the session overlap. Report shape matches and, for strong ones, the
least-squares raw->engineering fit with residuals across the full range.
"""
import sys, os
from collections import defaultdict, Counter
import numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from usbpcap import reassemble_frames

CAP = os.path.join(os.path.dirname(__file__), "../dumps/capture-w-logs.pcapng")
ENG = os.path.join(os.path.dirname(__file__), "../dumps/logs-engine.CSV")
DSG = os.path.join(os.path.dirname(__file__), "../dumps/logs-dsg.CSV")
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
        (b8 if (p[0] == 0xB8 and f["dir"] == "OUT") else
         b7 if (p[0] == 0xB7 and f["dir"] == "IN") else []).append((f["t"], blk))
    return b8, b7


def extract():
    """Return list of {label, t0abs, series[(t, data_bytes)]}."""
    b8, b7 = load()
    T0 = min(t for t, _ in b8 + b7)
    chreq, chrsp = defaultdict(list), defaultdict(list)
    for t, b in b8:
        chreq[ck(b)].append(b)
    for t, b in b7:
        chrsp[ck(b)].append((t, b))
    out = []
    for c, reqs in chreq.items():
        rsp = chrsp.get(c, [])
        if len(rsp) < 10:
            continue
        hdr = "%02x%02x%02x%02x" % c
        ks = tp_ks(reqs)
        if ks is not None:
            # full keystream: decode DID + data per response
            per = defaultdict(list)
            for t, b in rsp:
                d = bytes(b[i] ^ ks.get(i, 0) for i in range(16))
                if d[7] != 0x62:
                    continue
                did = (d[8] << 8) | d[9]
                n = d[6] & 0x0F
                per[did].append((t, d[10:7 + n]))
            for did, ser in per.items():
                if len(ser) >= 10:
                    out.append({"label": f"{hdr}/DID{did:04X}", "series": sorted(ser)})
        else:
            # two-time-pad: cluster by echoed DID cipher (off8,9), data via padding crib
            modal = Counter(reqs).most_common(1)[0][0]
            pos = [(t, r) for t, r in rsp if (r[7] ^ modal[7]) == 0x40]
            if len(pos) < 10:
                continue
            cl = defaultdict(list)
            for t, r in pos:
                cl[(r[8], r[9])].append((t, bytes([r[10] ^ modal[10], r[11] ^ modal[11]])))
            for echo, ser in cl.items():
                if len(ser) >= 10:
                    out.append({"label": f"{hdr}/ttp{echo[0]:02x}{echo[1]:02x}",
                                "series": sorted(ser)})
    return out, T0


def parse_csv(fn):
    L = [l.rstrip("\n") for l in open(fn, encoding="latin-1")]
    ide, unit = L[4].split(","), L[6].split(",")
    grp = L[2].split(",")
    data = [d for d in L[7:] if d.strip(",") != ""]
    out = []
    gi = 0
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
        if len(ser) < 8:
            continue
        u = unit[j].strip() if j < len(unit) else ""
        # group selector: G### is at position 2*(measurement index)+2 in row2
        out.append({"id": h.strip(), "unit": u, "series": ser})
    return out


FORMS = {
    "u8[0]": lambda d: d[0],
    "u8[1]": lambda d: d[1] if len(d) > 1 else d[0],
    "u16be": lambda d: (d[0] << 8) | d[1] if len(d) > 1 else d[0],
    "u16le": lambda d: (d[1] << 8) | d[0] if len(d) > 1 else d[0],
    "i16be": lambda d: (((d[0] << 8 | d[1]) ^ 0x8000) - 0x8000) if len(d) > 1 else d[0],
}


def align(rt, rv, csv, dt=0.4, maxlag=40):
    tr = np.asarray(rt, float); vr = np.asarray(rv, float)
    tc = np.asarray([t for t, _ in csv], float); vc = np.asarray([v for _, v in csv], float)
    tr = tr - tr[0]; tc = tc - tc[0]
    if vr.std() < 1e-6 or vc.std() < 1e-6:
        return None
    gr = np.arange(0, tc[-1], dt); ci = np.interp(gr, tc, vc)
    best = None
    for k in range(int(-maxlag / dt), int(maxlag / dt) + 1):
        ri = np.interp(gr, tr + k * dt, vr, left=np.nan, right=np.nan)
        m = ~np.isnan(ri)
        if m.sum() < 25 or ri[m].std() < 1e-6:
            continue
        r = np.corrcoef(ri[m], ci[m])[0, 1]
        if best is None or abs(r) > abs(best[0]):
            best = (r, k * dt, int(m.sum()))
    return best


def main():
    ents, T0 = extract()
    csvs = [("eng", m) for m in parse_csv(ENG)] + [("dsg", m) for m in parse_csv(DSG)]
    print(f"# {len(ents)} DID series ; {len(csvs)} CSV measurements")
    res = []
    for e in ents:
        s = e["series"]
        rt = [t for t, _ in s]
        for fn, fv in FORMS.items():
            try:
                rv = [fv(v) for _, v in s]
            except Exception:
                continue
            for src, m in csvs:
                b = align(rt, rv, m["series"])
                if b:
                    res.append((abs(b[0]), b[0], b[1], b[2], e["label"], fn, src, m["id"], m["unit"]))
    res.sort(reverse=True)
    print("\n# TOP matches:")
    for ar, r, lag, n, lab, fn, src, mid, u in res[:30]:
        print(f"  |r|={ar:.3f} r={r:+.3f} lag={lag:+5.1f} n={n:3d}  {lab:18s} {fn:6s}"
              f"  {src}/{mid} [{u}]")


if __name__ == "__main__":
    main()
