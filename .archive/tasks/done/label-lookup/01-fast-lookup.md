# label-lookup / 01 — fast label lookup (vag-db / vag-data)

**Subsystem:** label-lookup · **Crates:** `vag-db`, `vag-data` · **Wave:** 1 · **Depends:** none

## Goal
Make label-corpus lookup FAST. `vagcan info` resolves ECU part numbers / coding /
measuring-block ids to human component names on the hot path (naming engine, turbo,
gearbox). It must not full-scan or re-parse per lookup.

## Context
- `vag-db` = SQLite cache (rusqlite, bundled) over the label corpus; `vag-data` =
  parsers/decoders (.lbl/.clb/.rod) + `LabelDb` lookup layer with REDIRECT resolution.
- Explore the current lookup path first: how `LabelDb`/vag-db resolves a part number
  today (indices? prepared statements? in-memory? per-call parse?). Document the
  current cost, then optimize.

## Deliverables
- Identify the hot lookup(s) the info command needs: part-number → label file, and
  measuring-block / coding → human name. Make each **O(log n) or O(1)**:
  - SQLite: add the right **indices** on lookup keys; use **prepared statements**
    (cached), open the DB once and reuse the connection.
  - OR preload the needed slice into an in-memory map (`HashMap`) at startup if the
    corpus subset is small enough — measure which wins.
  - Resolve REDIRECT chains without repeated round-trips (single query or memoized).
- Keep the lookup **sync** (CPU-bound; it will be called from blocking-friendly
  context, not the async reactor hot loop).
- A **criterion benchmark** (or a simple timed harness if criterion is too heavy)
  proving the lookup is fast (target: sub-millisecond per part-number lookup on the
  existing corpus). Commit the benchmark.

## TDD
1. Write a test that does N lookups against a fixture corpus and asserts correct
   resolution (part number → expected label/name), incl. a REDIRECT case.
2. Add the benchmark; record before/after numbers in the task's report.
3. Optimize; keep the correctness test green.

## Done criteria
- Lookups correct (test green) and fast (benchmark shows sub-ms per lookup, or
  document the achieved number + why it's the floor).
- `cargo test -p vag-db -p vag-data` green; clippy `-D warnings` clean.
- No API break to existing decoders unless justified; if `LabelDb`'s signature
  changes, note it in the report (vin-info will consume it).

## Interfaces (Produces)
- The fast `LabelDb` lookup API (consumed by `vin-info` to name components).
