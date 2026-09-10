#!/usr/bin/env python3
"""Write reference-survey.jsonl: the reference car's fifteen confirmed faults in
the shape `vagcan dev survey` records, for `vagcan faults --from`.

Every value is transcribed from the archive, not read off a car:
`.archive/research/labels/fault-naming-hop.md` §12.1 (the codes and each
unit's F19E) and `.archive/research/car/other-ecus.md` §2 (the F1A2 of the
three units VCDS was seen to identify). Units whose F1A2 the archive does not
record carry none, which is the shape a survey has when the identifier was
not answered. The recorded survey those figures came from,
`research/dumps/survey-parked.jsonl`, is gitignored and no longer on disk.
"""
import json

UNITS = [
	# request, short number, F19E, F1A2, confirmed codes
	("70C", "16", "EV_SMLSVALEOMQBLRH", "001007", ["047120"]),
	("713", "03", "EV_Brake1UDSContiMK100ESP", None, ["00004B", "00005B", "000129"]),
	("70A", "10", "EV_EPHVA14AU3700000", None, ["D01721", "D01722", "D0172E", "D0172F", "D01732"]),
	("712", "44", "EV_SteerAssisMQB", None, ["004F04", "004D04"]),
	("70E", "09", "EV_BCMMQB", "017001", ["000107", "000213", "060901"]),
	("710", "19", "EV_GatewNF", None, ["010405"]),
]

with open("reference-survey.jsonl", "w") as out:
	for request, unit, odx, version, codes in UNITS:
		ident = [{"did": "F19E", "data": odx.encode().hex().upper()}]
		if version:
			ident.append({"did": "F1A2", "data": version.encode().hex().upper()})
		row = {
			"request": request,
			"unit": unit,
			"ident": ident,
			"dtcs": [{"code": c, "status": "08"} for c in codes],
		}
		out.write(json.dumps(row) + "\n")
