# roles-sweep — `measure`'s roles over every engine and gearbox of an ODIS project

What it is for, what it takes and what it writes: the doc comment at the top of
[`src/main.rs`](src/main.rs). It ran on 2026-09-28 against the SK37X project (385 engine and 10
gearbox variants, 792 configurations with and without the reference car's other units) while
PR #17 was reviewed.

The variant lists, from the project's cache:

```
sqlite3 "file:$HOME/.vagcan/data/SK37X/cache.sqlite?mode=ro" \
  "SELECT DISTINCT variant FROM reading WHERE variant LIKE 'EV_ECM%'" > engines.txt
sqlite3 "file:$HOME/.vagcan/data/SK37X/cache.sqlite?mode=ro" \
  "SELECT DISTINCT variant FROM reading WHERE variant LIKE 'EV_TCM%'" > gearboxes.txt
```

A comparison is two runs from two checkouts of the crate's tree:

```
cargo run --release -- ~/.vagcan/data/SK37X/cache.sqlite engines.txt gearboxes.txt > head.txt
# the same from the other checkout > base.txt
diff <(sort base.txt) <(sort head.txt)
```

Not a workspace member, and CI does not build it; it reads the machine's own ODIS cache.
