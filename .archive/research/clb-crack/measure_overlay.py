#!/usr/bin/env python3
"""Focused overlays: for a chosen (DID,form,measurement) print the resampled raw
and CSV value side-by-side at the best lag, plus the linear fit. Lets us eyeball
whether a correlation is a real tracking or a windowed coincidence.

Also: find the single GLOBAL lag that maximises summed |r| across all 7 measures
paired to their best DID (all channels share one clock)."""
import sys, os
from collections import defaultdict
import numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import measure_coolant as M

DT = 0.5


def merged_series(series, did):
    return sorted(series[did])


def overlay(series, did, fn, m, lag):
    s = merged_series(series, did)
    fv = M.FORMS[fn]
    rt = np.asarray([t for t, _ in s], float); rt -= rt[0]
    rv = np.asarray([fv(v) for _, v in s], float)
    tc = np.asarray([t for t, _ in m["series"]], float); tc -= tc[0]
    vc = np.asarray([v for _, v in m["series"]], float)
    g = np.arange(0, tc[-1] + 1e-9, DT)
    cg = np.interp(g, tc, vc)
    rg = np.interp(g + lag, rt, rv, left=np.nan, right=np.nan)
    mm = ~np.isnan(rg)
    x, y, gg = rg[mm], cg[mm], g[mm]
    r = np.corrcoef(x, y)[0, 1]
    A = np.vstack([x, np.ones_like(x)]).T
    (a, b), *_ = np.linalg.lstsq(A, y, rcond=None)
    pred = a * x + b
    r2 = 1 - ((y - pred) ** 2).sum() / ((y - y.mean()) ** 2).sum()
    print(f"  DID{did:04X} {fn} vs {m['id']}[{m['unit']}] lag={lag:.1f}: "
          f"r={r:+.3f} R2={r2:+.3f} a={a:.5g} b={b:.5g}")
    # print every ~4th grid point: time, raw, actual, pred
    for i in range(0, len(gg), max(1, len(gg) // 20)):
        print(f"      t={gg[i]:5.1f} raw={x[i]:8.1f} actual={y[i]:7.2f} pred={pred[i]:7.2f}")
    return r, r2, a, b


def global_lag(series, csvs):
    lags = np.arange(30, 65, 0.5)
    best = None
    for lag in lags:
        tot = 0
        for m in csvs.values():
            bestr = 0
            for did in series:
                s = merged_series(series, did)
                rt = np.asarray([t for t, _ in s], float); rt -= rt[0]
                for fn, fv in M.FORMS.items():
                    try:
                        rv = np.asarray([fv(v) for _, v in s], float)
                    except Exception:
                        continue
                    if rv.std() < 1e-9:
                        continue
                    tc = np.asarray([t for t, _ in m["series"]], float); tc -= tc[0]
                    vc = np.asarray([v for _, v in m["series"]], float)
                    g = np.arange(0, tc[-1] + 1e-9, DT)
                    cg = np.interp(g, tc, vc)
                    rg = np.interp(g + lag, rt, rv, left=np.nan, right=np.nan)
                    mk = ~np.isnan(rg)
                    if mk.sum() < 25 or rg[mk].std() < 1e-9:
                        continue
                    r = abs(np.corrcoef(rg[mk], cg[mk])[0, 1])
                    bestr = max(bestr, r)
            tot += bestr
        if best is None or tot > best[1]:
            best = (lag, tot)
    return best


def main():
    series, _ = M.extract()
    csvs = {m["id"]: m for m in M.parse_csv()}
    gl = global_lag(series, csvs)
    print(f"# GLOBAL best lag = {gl[0]:.1f}s (summed |r| = {gl[1]:.2f})\n")
    L = gl[0]
    print("# overlays at global lag for physically-motivated pairings:")
    tests = [
        (0xA03B, "u16be", "IDE00405"),  # RPM?
        (0xA0EF, "u16be", "IDE00405"),
        (0xA03B, "u16be", "IDE00075"),  # speed?
        (0xA0EF, "u16be", "IDE00075"),
        (0x7458, "u8[0]", "IDE00075"),
        (0x7450, "u8[0]", "IDE00025"),  # coolant?
        (0x7410, "u8[0]", "IDE00025"),
        (0x7419, "u8[0]", "IDE00025"),
    ]
    for did, fn, mid in tests:
        print()
        overlay(series, did, fn, csvs[mid], L)


if __name__ == "__main__":
    main()
