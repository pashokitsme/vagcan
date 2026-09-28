#!/usr/bin/env python3
"""Pub items that nothing but tests uses any more — what the dead-code check cannot see.

FOR: after a removal, finding what it orphaned. `RUSTFLAGS="--force-warn dead_code" cargo check
--workspace` does not warn on a library crate's pub items, so a pub function whose last caller
went stays silent. Found, after `dev survey` was removed on 2026-09-28,
`AsyncUdsClient::read_data_by_identifiers` and `anomaly::Monitor::seed`/`heard`/`silent_span`
(todo/label-lookup/04 item 3). Written by a review subagent; kept per the cleanup skill's
2026-09-22 and 2026-09-27 rules.

IN: one directory holding two checkouts, `base/` (before the change) and `head/` (after), e.g.
    git worktree add /tmp/o/base <before>; git worktree add /tmp/o/head <after>
    python3 research/tools/orphans.py /tmp/o
OUT: stdout — every pub item with no use outside tests at head: its definition site and its use
counts, prod and test, at base and head; "<-- CHANGED" marks the ones that had a prod use at base
(what the change orphaned). Word counts, not name resolution: a name used for something else
elsewhere hides an orphan, and a mention in a comment is not a use. Each hit is a candidate — the
cleanup skill's Phase 4 decides whether it is dead or a missing call site.
"""

import os
import re
import sys
from collections import defaultdict

ROOT = sys.argv[1]
TREES = {"base": os.path.join(ROOT, "base"), "head": os.path.join(ROOT, "head")}

DEF = re.compile(
    r"^\s*pub(?:\([^)]*\))?\s+(?:async\s+)?(?:const\s+fn|unsafe\s+fn|fn|const|static|struct|enum|trait|type|mod)\s+([A-Za-z_][A-Za-z0-9_]*)"
)
WORD = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")


def rs_files(tree):
    for top in ("crates", "research"):
        base = os.path.join(tree, top)
        for dirpath, dirnames, filenames in os.walk(base):
            if "target" in dirnames:
                dirnames.remove("target")
            for f in filenames:
                if f.endswith(".rs"):
                    yield os.path.join(dirpath, f)


def split(path, text):
    lines = text.split("\n")
    rel_is_test = "/tests/" in path or path.endswith("/tests.rs") or "/benches/" in path
    if rel_is_test:
        return [], lines
    cut = len(lines)
    for i, line in enumerate(lines):
        if line.strip().startswith("#[cfg(test)]"):
            # next non-empty line starts a mod
            for j in range(i + 1, min(i + 4, len(lines))):
                if lines[j].strip().startswith("mod ") or lines[j].strip().startswith("pub mod ") or lines[j].strip().startswith("pub(crate) mod "):
                    cut = i
                    break
            if cut != len(lines):
                break
    return lines[:cut], lines[cut:]


def scan(tree):
    defs = defaultdict(list)  # name -> [(file, lineno)]
    prod = defaultdict(int)
    test = defaultdict(int)
    for path in rs_files(tree):
        try:
            text = open(path, encoding="utf-8", errors="replace").read()
        except OSError:
            continue
        p, t = split(path, text)
        rel = os.path.relpath(path, tree)
        for i, line in enumerate(p):
            m = DEF.match(line)
            if m:
                defs[m.group(1)].append((rel, i + 1))
            # strip line comments (keep doc links? no: a doc mention is not a use)
            code = line.split("//")[0]
            for w in WORD.findall(code):
                prod[w] += 1
        for line in t:
            code = line.split("//")[0]
            for w in WORD.findall(code):
                test[w] += 1
    return defs, prod, test


base_defs, base_prod, base_test = scan(TREES["base"])
head_defs, head_prod, head_test = scan(TREES["head"])

rows = []
for name, sites in head_defs.items():
    # definition itself counts once per site in prod
    uses_head = head_prod[name] - len(sites)
    uses_base = base_prod[name] - len(base_defs.get(name, []))
    if uses_head <= 0:
        rows.append((name, sites, uses_base, uses_head, head_test[name], base_test[name]))

rows.sort(key=lambda r: r[1][0])
print("pub items with no non-test use at HEAD (name, def, base-prod-uses, head-prod-uses, head-test-uses, base-test-uses):")
for name, sites, ub, uh, th, tb in rows:
    flag = "  <-- CHANGED" if ub > 0 else ""
    print(f"{name:40} {sites[0][0]}:{sites[0][1]:<6} base={ub:<3} head={uh:<3} test_head={th:<3} test_base={tb:<3}{flag}")
