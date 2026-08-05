---
name: cleanup
description: Use when this project needs a consolidation pass — a context window is about to reset, the docs no longer match the code, research files have piled up, dead command paths are suspected, or the next goals have gone unclear.
---

# Cleanup — a consolidation pass over vagcan

## Overview

A cleanup pass is **lossy by default**, and this project cannot afford loss: most of
what it knows was measured on one car, once, and several of its most valuable
documents are records of things that did **not** work. Tidying is the operation most
likely to throw those away.

So the core principle is: **nothing leaves without a destination.** Not deleted —
moved. Research goes to `archive/`, a one-shot tool becomes a subcommand, a fact goes
to the file that owns that kind of fact. The only thing a cleanup pass may actually
delete is code that no longer compiles into anything.

## Do the phases in order

Verification comes first because every later phase acts on claims, and in this
repository the claims have been wrong.

The phases are: verify, route the facts, archive research, retire code, re-verify the
skills, state the next goals.

### Phase 1 — verify before consolidating

1. `cargo test --workspace` and `cargo clippy --workspace --all-targets`. Green
   before, green after. A cleanup pass that changes behaviour is not a cleanup pass.
2. `git status` must be clean, and **stays** clean for anyone else: stage by explicit
   path, never `git add -A`, `git add .` or `git commit -a`. Twice in this repo a
   blanket add swept a subagent's uncommitted work into an unrelated commit
   (`dcaee71`, `dec37a1`).
3. Take every status claim in `todo/README.md` and check it against the code with
   `grep`, not against memory. Real drift found this way: the file said `faults` was
   "in the working tree, not yet merged" long after it was merged.

### Phase 2 — route the facts to the files that own them

Consolidation is not summarising into one place. It is putting each kind of fact in
the file that survives a context reset **for that kind of fact**:

| what was learned | where it belongs |
|---|---|
| a proven measurement — read address, scaling, unit | `catalogs/vehicles/<part number>.json` |
| how something was proven, or refuted, and with what data | `research/<topic>.md` |
| current state, milestones, what to do next | `todo/README.md` |
| goal, stack, workflow | `todo/GOAL.md` |
| anything that risked or damaged the car | `SAFETY.md` |
| a rule the tool must obey forever | `CLAUDE.md` |
| a path nobody should retry | `archive/research/`, plus one line under "Dead and archived" |

Rules for the writing itself:

- **Absolute dates.** "Last session", "recently" and "now" mean nothing after a
  reset. Every status line carries a date.
- **State the evidence, not the conclusion alone.** "Proven against X, N points,
  R² = …" survives; "confirmed" does not.
- **One statement of a fact.** Two files claiming the same thing will disagree within
  a month. Pick the owner from the table, and cross-reference from the others.

### Phase 3 — archive research, and re-check the refutation first

Before moving a research file to `archive/`, establish which it is:

- **A dead end that is still true** — archive it. It is what stops a future session
  paying for the same negative result twice. Keep the reasoning; a bare "does not
  work" is worthless.
- **A dead end that died of a bug in our own tooling** — do not archive. Re-run it.
  The recorded refutation "STRUC does not carry the fine scalings" rested on
  `glyphs::decode` silently dropping the decimal point; the path was alive the whole
  time.

So: for every refutation about to be archived, name the reason it failed and check
whether that reason still holds today. If the reason was a defect in this codebase,
the refutation is void until re-run.

Never delete a research file. Archived, it costs a directory entry; deleted, the next
session repeats the experiment.

### Phase 4 — retire code by relocation

- **A one-shot tool is not dead code.** The `.rod` key search and the corpus dump
  each ran once and produced a committed artefact, and each keeps its value for the
  next corpus, on another machine, or when an artefact is doubted. They were retired
  into `vagcan vcds rod` and `vagcan vcds corpus` rather than into `git rm`; do the
  same with the next one.
- **Every one-shot tool states three things in its help text**, because by the time
  it is needed again nobody will remember them: **what it is for** (and what it
  produced last time, so the artefact can be recognised), **what it expects on
  input** — the exact file or directory, not "a path" — and **what it writes on
  output**, named. A tool whose help does not answer all three is not retired
  properly, whatever directory it ends up in.
- **Two tools doing one job is a defect, not redundancy.** Merge them into the
  survivor, or delete the older one outright — never leave both. Two commands for one
  job means the next session picks the wrong one and gets a subtly different answer.
  Pick the survivor by capability, not by age: whichever covers the other's flags.
  If the older one has a mode the survivor lacks, port that mode first, then delete —
  the merge is not finished while a capability is stranded.
- **Genuinely dead** means: no caller outside its own tests, **and** the reason it was
  kept has since been refuted. Both halves, and the second is the one that gets
  skipped. `mwb.rs` is the worked example of failing it: exported from `lib.rs`,
  called by nothing but its own tests — and still not dead, because the MWB→TTTEXT
  name join it is held for is recorded as a live prediction
  (`research/labels/label-linkage.md` §5, `todo/README.md`). Uncalled is half a case.
- **The top level of the CLI is for commands used with the car in front of you.**
  That is the whole test: if it needs an adapter and a running vehicle, it is a
  top-level command. Everything that reads static files — a VCDS installation, a
  recovered catalog, a recording this tool made earlier — belongs in a subcommand
  group, however useful it is. A top level crowded with offline analysis is a top
  level nobody can scan while standing at an open driver's door. Group by **what the
  input is**, not by how the code is organised: files that came from VCDS are one
  group, recordings we made are another.
- **Never simplify a data-driven path into a table in Rust.** `CLAUDE.md` forbids
  car-specific data in code, and cleanup is exactly when someone "tidies" a JSON
  lookup into a `match`. Cleanup should move data *out* of code, never in.
- **Never remove a safety guard while consolidating.** `require_stationary` and the
  read-only UDS allowlist are not boilerplate. If a command resembles a sweep and is
  unguarded, the cleanup finding is "guard it", never "it has been fine so far".

### Phase 5 — re-verify the skills against the program

The skills under `.claude/skills/` are documentation of a moving target, and they
fail in a way ordinary docs do not: an agent reading a stale skill runs a command
that no longer exists, gets an error, and starts improvising. A wrong skill is worse
than no skill.

So a cleanup pass ends by checking them against the binary, not against memory:

1. Extract every `vagcan …` invocation from every skill, and run each one's `--help`.
   A command that does not resolve is a defect found; fix the skill in the same
   commit that moved the command.
2. Re-read **this** file. A rule that has been superseded — a phase that names a
   command by its old path, a hazard that no longer exists — gets updated here, or
   the next pass enforces something untrue.
3. If the pass established a new rule, write it down while it is still an argument
   you remember making. A rule with no reason attached is one the next session talks
   itself out of.

This phase is not optional and is not "documentation polish". It is the phase that
keeps every other phase from decaying.

### Phase 6 — say what to do next, and why it is next

End with a short ordered list. Each item names the goal from `todo/GOAL.md` it moves,
and says whether the car is needed — that single fact decides what can be done
tonight. An item nobody can start without a drive belongs in its own section, not
mixed in.

## What a cleanup pass produces

Five things, and no more:

1. Commits that move files and delete nothing of substance, each staged by path.
2. `todo/README.md` accurate as of today's date, with the milestone table matching
   what the code actually does.
3. New or moved files under `archive/`, each still carrying its reasoning.
4. Skills under `.claude/skills/` whose every command was just run against `--help`.
5. The next-goals list from Phase 6, in the answer as well as in the file.

## Never

- `git add -A`, `git add .`, `git commit -a`.
- Delete a research file, a capture, a recording, or a catalog row measured on the
  car. None of it can be re-collected without the car.
- Delete a one-shot tool. Move it.
- Trust a recorded refutation without checking that its cause still holds.
- Remove a safety guard, or add a write service, while "consolidating".
- Update a status line without a date.

## Rationalization table

| Excuse | Reality |
|---|---|
| "This research file is obsolete" | Obsolete research is the archive's whole purpose. Move it. |
| "The tool already ran, its output is committed" | Then it will need to run again on the next corpus. Relocate it, and document its input and output while you still remember them. |
| "Both tools work, leaving both is harmless" | Two commands for one job means the next session picks the wrong one. Merge or delete. |
| "Nothing calls this, so it is dead" | Uncalled is half the test. The other half is that its reason has been refuted. |
| "The doc says this path is refuted" | Check what refuted it. Once here it was our own bug. |
| "`git add -A` is faster, everything is mine" | It was not, twice. Stage by path. |
| "This guard is over-cautious for a read-only tool" | The read-only tool cost this car its power steering. Read `SAFETY.md`. |
| "I'll fold this JSON into a small table, it is cleaner" | That is the one thing `CLAUDE.md` forbids outright. |
| "Tests were green before, no need to re-run" | The pass is judged by green after. Run them. |

## Red flags — stop

- About to type `git add -A`.
- About to run `git rm` on anything under `research/`, `catalogs/` or `crates/*/bin/`.
- Writing "recently", "last session", "currently" into a status document.
- A cleanup diff that touches `safety.rs`, `address.rs` or the UDS allowlist.
- `cargo test` not run since the pass began.
