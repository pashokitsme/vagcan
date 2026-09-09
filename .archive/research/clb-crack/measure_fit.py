#!/usr/bin/env python3
"""Rigorous raw->engineering fit for coolant-rpm-speed capture.

Hypothesis: capture_rel_time tr = csv_rel_time tc + LAG (free LAG, scanned).
For each DID x form x CSV-measurement: scan LAG, resample both onto the CSV time
grid, Pearson r + least-squares linear fit (value = a*raw + b), pick the LAG that
maximises R^2. Report a,b,R^2,resid,n and sample (raw->value) pairs.

Analysis-only; dumps gitignored.
"""
import sys, os
from collections import defaultdict
import numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import measure_coolant as M

DT = 0.5


def resample(tr, vr, tc, vc, lag):
    """Return (raw_on_grid, val_on_grid) over the CSV span at this lag."""
    g = np.arange(0, tc[-1] + 1e-9, DT)
    cg = np.interp(g, tc, vc)
    # raw at capture-time g+lag
    rg = np.interp(g + lag, tr, vr, left=np.nan, right=np.nan)
    m = ~np.isnan(rg)
    return rg[m], cg[m], int(m.sum())


def best_fit(rt, rv, csv, lags):
    tr = np.asarray(rt, float); tr = tr - tr[0]
    vr = np.asarray(rv, float)
    tc = np.asarray([t for t, _ in csv], float); tc = tc - tc[0]
    vc = np.asarray([v for _, v in csv], float)
    if vr.std() < 1e-9 or vc.std() < 1e-9:
        return None
    best = None
    for lag in lags:
        x, y, n = resample(tr, vr, tc, vc, lag)
        if n < 25 or x.std() < 1e-9:
            continue
        r = np.corrcoef(x, y)[0, 1]
        A = np.vstack([x, np.ones_like(x)]).T
        (a, b), *_ = np.linalg.lstsq(A, y, rcond=None)
        pred = a * x + b
        ss_res = ((y - pred) ** 2).sum()
        ss_tot = ((y - y.mean()) ** 2).sum()
        r2 = 1 - ss_res / ss_tot if ss_tot > 0 else -9
        resid = np.sqrt(ss_res / n)
        if best is None or r2 > best["r2"]:
            best = dict(lag=lag, r=r, a=a, b=b, r2=r2, resid=resid, n=n)
    return best


def main():
    series, _ = M.extract()
    csvs = M.parse_csv()
    lags = np.arange(0, 90, DT)
    res = []
    for did in sorted(series):
        s = sorted(series[did])
        rt = [t for t, _ in s]
        for fn, fv in M.FORMS.items():
            try:
                rv = [fv(v) for _, v in s]
            except Exception:
                continue
            if len(set(rv)) < 2:
                continue
            for m in csvs:
                bf = best_fit(rt, rv, m["series"], lags)
                if bf:
                    res.append((bf["r2"], did, fn, m, bf))
    res.sort(key=lambda x: -x[0])
    print("# top fits by R^2 (value = a*raw + b, lag = capture-leads-CSV seconds)")
    seen = set()
    for r2, did, fn, m, bf in res[:40]:
        print(f"  R2={r2:+.3f} r={bf['r']:+.3f} DID{did:04X} {fn:6s} -> {m['id']}[{m['unit']}] "
              f"a={bf['a']:.5g} b={bf['b']:.5g} lag={bf['lag']:.1f} resid={bf['resid']:.3f} n={bf['n']}"
              f"  {m['name'][:22]}")

    # Detailed dump for the best fit per measurement
    print("\n# best fit per CSV measurement:")
    for m in csvs:
        cand = [x for x in res if x[3]["id"] == m["id"]]
        if not cand:
            continue
        r2, did, fn, mm, bf = cand[0]
        print(f"\n## {m['id']} [{m['unit']}] {m['name'][:30]}")
        print(f"   best: DID{did:04X} {fn} R2={r2:+.3f} a={bf['a']:.6g} b={bf['b']:.6g} "
              f"lag={bf['lag']:.1f} resid={bf['resid']:.3f}")
        # show sample raw->predicted vs actual at aligned grid
        s = sorted(series[did]); rt = [t for t, _ in s]
        fv = M.FORMS[fn]; rv = [fv(v) for _, v in s]
        tr = np.asarray(rt, float) - rt[0]; vr = np.asarray(rv, float)
        tc = np.asarray([t for t, _ in m["series"]], float); tc -= tc[0]
        vc = np.asarray([v for _, v in m["series"]], float)
        x, y, n = resample(tr, vr, tc, vc, bf["lag"])
        # sample 8 spread points
        idx = np.linspace(0, len(x) - 1, min(8, len(x))).astype(int)
        for i in idx:
            print(f"     raw={x[i]:8.1f} -> pred={bf['a']*x[i]+bf['b']:7.2f}  actual={y[i]:7.2f}")


if __name__ == "__main__":
    main()
