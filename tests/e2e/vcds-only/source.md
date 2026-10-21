# A source asked for and not set up

With a VCDS installation alone, `--source odis` is refused before the adapter is opened —
never answered from VCDS instead.

```console
$ vagcan --source odis faults
? failed
error: --source odis: no ODIS project is set up

help: read one: `vagcan setup <ODIS project>`
help: or read a VCDS installation instead: `--source vcds`

$ vagcan --source vcds faults --unit 01
? success
...

```
