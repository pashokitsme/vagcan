# Independent check (main session, not the agent's scripts): gearbox MWB row n -> RM row (n + shift)
# decoded under srand(key); compare DID/scaling with the car-proven rows.
import json, sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from alphabet import decoder
# scratch dir holding dump/ and out/: argv[1], or $ODIS_CRIB, or ../scratch beside the scripts
S = (sys.argv[1] if len(sys.argv) > 1 else None) or os.environ.get("ODIS_CRIB") \
    or os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "scratch")
def rows(path):
    raw = open(path, 'rb').read().decode('latin-1')
    return [r for r in raw.split('\r\n') if len(r) > 7 and r[:6].isdigit() and r[6] == ',']
rm = rows(S + '/dump/RM/MWB.bin')
unit = rows(S + '/dump/EV_TCMDQ200021/MWB.bin')
txt = {int(r[:6]): r[7:] for r in rows(S + '/dump/TTTEXT/TXT.bin')}
proven = json.load(open(os.path.expanduser('~/.vagcan/data/SK37X/measurements/0CW300041G.json')))['defs']
print('RM rows', len(rm), 'gearbox MWB rows', len(unit))
def rm_row(i):
    k = int(rm[i][:6]); return k, decoder(k)(rm[i][7:]).split(',')
for shift in (-1, 0, 1):
    hits = 0; out = []
    dids = {}
    for r in unit:
        n = int(r[:6]); i = n + shift
        if not (0 <= i < len(rm)): continue
        k, f = rm_row(i)
        if f[0].isdigit(): dids.setdefault(int(f[0]), []).append((n, k, f))
    for p in proven:
        d = p['address']['Uds']
        if d in dids: hits += 1
    print(f'shift {shift:+d}: proven DIDs present among the rows the gearbox selects: {hits}/{len(proven)}')
    if shift == -1:
        for p in proven:
            d = p['address']['Uds']; lin = p['scaling'].get('Linear')
            for n, k, f in dids.get(d, [])[:2]:
                f3, f4, f5 = (float(x) if x else 0.0 for x in (f[3], f[4], f[5]))
                fac = f4 / f5 if f5 else None; off = f3 / f5 if f5 else None
                t = decoder(k)(txt.get(k, ''))
                le = bool(int(f[2] or 0) & 64); sg = bool(int(f[2] or 0) & 128)
                print(f"  {d:04X} {p['name'][:28]:28} proven {p['raw_form']:7} x{lin['factor'] if lin else '-'} | "
                      f"row {n} key {k} [{t[:34]}] len {f[9]} LE {le} signed {sg} x{fac} +{off} unit#{f[6]}")
