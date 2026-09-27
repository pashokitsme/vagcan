"""Compare VCDS installs: every .rod file's sections and their regime (classic / shifted / tea).

FOR: README §6a — whether another release (or the Russian install) opens what one release keeps
shifted. It does not: the regime is a property of the file (2026-09-28: MWB changed regime in
4 of 5,021 files between EN 25.12 and EN 26.3, 1 of 2,910 between RU and EN 26.3).
IN:  name=<UDS_EV dir> pairs, e.g.
       en2512=vendor/vcds-en/UDS_EV  ru=vendor/vcds-ru/UDS_EV  en263=~/vcds-en/UDS_EV
OUT: prints the comparison; writes $ODIS_CRIB/out/census_installs.json (per install, per file:
     md5, size, [(tag, regime)]). Reads only the first block of each section — seconds.
"""
import collections, hashlib, json, os, re, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import rodlib  # noqa: E402
from common import S  # noqa: E402


def census(uds):
	files = {}
	for f in sorted(os.listdir(uds)):
		if not f.lower().endswith(".rod"):
			continue
		data = open(os.path.join(uds, f), "rb").read()
		secs = []
		for tag, comp, plain, cipher in rodlib.sections(data):
			kind, _ = rodlib.classify(tag, comp, plain, cipher) if cipher is not None else ("bad", None)
			secs.append((tag, kind))
		files[f] = {"md5": hashlib.md5(data).hexdigest(), "size": len(data), "secs": secs}
	return files


def regime(info):
	kinds = {k for _, k in info["secs"]}
	if "shifted" in kinds:
		return "shifted"
	return "classic" if "classic" in kinds else "tea/other"


def main(pairs):
	installs = {}
	for p in pairs:
		name, d = p.split("=", 1)
		installs[name] = census(os.path.expanduser(d))
	names = list(installs)
	for n in names:
		files = installs[n]
		ev = [v for f, v in files.items() if f.startswith("EV_")]
		shifted = sum(regime(v) == "shifted" for v in files.values())
		print(f"{n}: {len(files)} .rod, any section shifted {shifted} ({shifted / len(files):.0%}); EV_ {len(ev)}, shifted {sum(regime(v) == 'shifted' for v in ev)}")
	for i, a in enumerate(names):
		for b in names[i + 1:]:
			common = set(installs[a]) & set(installs[b])
			same = sum(installs[a][f]["md5"] == installs[b][f]["md5"] for f in common)
			print(f"{a} & {b}: {len(common)} files by name, byte-identical {same}")
			for tag in ("MWB", "INC"):
				m = collections.Counter()
				for f in common:
					x = dict(installs[a][f]["secs"]).get(tag)
					y = dict(installs[b][f]["secs"]).get(tag)
					if x in ("classic", "shifted") and y in ("classic", "shifted"):
						m[(x, y)] += 1
				print(f"  {tag}: " + ", ".join(f"{x}->{y} {c}" for (x, y), c in sorted(m.items())))
	for n in names:
		others = set().union(*(set(installs[o]) for o in names if o != n)) if len(names) > 1 else set()
		only = sorted(set(installs[n]) - others)
		base = lambda f: re.sub(r"(_[A-Z]{2}\d{2}[A-Z0-9]*)?(_[A-Z]?\d+)?\.rod$", "", f, flags=re.I)
		other_bases = {base(f) for o in names if o != n for f in installs[o]}
		print(f"only in {n}: {len(only)}, of them with the same base name elsewhere {sum(base(f) in other_bases for f in only)}")
	globals_ = sorted({f for n in names for f in installs[n] if not re.match(r"(EV|IV|GV|CRFT\d*)_", f)})
	for f in globals_:
		print(f"  {f}: " + " | ".join(f"{n} {[k for _, k in installs[n][f]['secs']] if f in installs[n] else '-'}" for n in names))
	out = os.path.join(S, "out", "census_installs.json")
	os.makedirs(os.path.dirname(out), exist_ok=True)
	json.dump(installs, open(out, "w"))
	print("wrote", out)


if __name__ == "__main__":
	if len(sys.argv) < 2:
		sys.exit(__doc__)
	main(sys.argv[1:])
