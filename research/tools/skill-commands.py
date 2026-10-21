#!/usr/bin/env python3
"""Every `vagcan …` command and flag the skills name, run against the binary's --help.

FOR: the cleanup skill's Phase 5. A shell loop over the extracted commands gave a false
"MISSING" on two passes running (zsh does not word-split `$c`, so `vagcan $c --help` passes
`vcds dump` as one argument); this splits the words itself. It also checks flags: a removed flag
leaves its subcommand resolving (`faults --from` went on 2026-09-28 and `faults` still parsed),
so a subcommand check alone says nothing about it. First run on 2026-10-21: 16 commands and every flag the
skills name resolved, nothing wrong; planted `dev survey`, `nosuch` and `faults --from` were caught.

IN: the path to a `vagcan` built from the tree being checked — not a test build, which prints a
note on every run: `cargo build -p vag-cli`, or `just e2e`, which leaves one at
`target/e2e/vagcan`. Run from the repository root; it reads `.claude/skills/*/SKILL.md`.
    python3 research/tools/skill-commands.py target/e2e/vagcan
OUT: stdout — every command prefix that resolved, then each problem as
`MISSING <skill> vagcan <words>` (a plain word where a command group takes a subcommand,
and clap refuses it) or
`FLAG <skill> vagcan <command> <flag>` (the flag is in neither that command's help nor the
top level's). Exit status 1 when there is a problem. `VAGCAN_HOME` is pointed at an empty
temporary directory, so no run reads `~/.vagcan`.
"""

import glob
import os
import re
import subprocess
import sys
import tempfile

BIN = sys.argv[1]
ENV = dict(os.environ, VAGCAN_HOME=tempfile.mkdtemp(prefix="skill-commands-"))
HELP = {}


def help_of(cmd):
	"""`vagcan <cmd> --help`'s text, or None when clap refuses the words."""
	key = tuple(cmd)
	if key not in HELP:
		run = subprocess.run([BIN, *cmd, "--help"], capture_output=True, text=True, env=ENV)
		HELP[key] = run.stdout if run.returncode == 0 else None
	return HELP[key]


def command_of(tokens):
	"""The longest run of leading words that clap takes as a command."""
	cmd = []
	for token in tokens:
		if not re.fullmatch(r"[a-z][a-z0-9-]*", token) or help_of(cmd + [token]) is None:
			break
		cmd.append(token)
	return cmd


top = help_of([]) or ""
resolved, problems = set(), []
for skill in sorted(glob.glob(".claude/skills/*/SKILL.md")):
	for line in open(skill, encoding="utf-8"):
		# Inside backticks: a command as typed. Outside them a "vagcan" is prose.
		for span in re.findall(r"`vagcan ([^`]+)`", line):
			tokens = span.split()
			cmd = command_of(tokens)
			rest = tokens[len(cmd) :]
			group = cmd == [] or "\nCommands:" in (help_of(cmd) or "")
			# A plain word where a command takes subcommands is one clap refused: `dev survey`.
			if rest and re.fullmatch(r"[a-z][a-z0-9-]*", rest[0]) and group:
				problems.append(f"MISSING {skill} vagcan {span}")
				continue
			resolved.add(" ".join(cmd))
			text = (help_of(cmd) or "") + top
			for token in tokens:
				if token.startswith("--"):
					flag = token.split("=")[0].rstrip(".,;:)")
					if flag not in text:
						problems.append(f"FLAG {skill} vagcan {' '.join(cmd)} {flag}")

for cmd in sorted(resolved):
	print(f"  vagcan {cmd}".rstrip())
for problem in sorted(set(problems)):
	print(problem)
sys.exit(1 if problems else 0)
