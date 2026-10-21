# What the VCDS store holds

`dev vcds stats` is a card: the cache, its totals, then its tables and the rows of VCDS's
other registries by section.

```console
$ vagcan dev vcds stats
? success
label cache ~/.vagcan/vcds/cache.sqlite
7 tables · 3 rows

TABLES
  label_file      0
  measurement     0
  redirect        0
  adaptation      0
  long_coding     0
  registry_row    3
  registry_level  0

REGISTRY ROWS
  FFMUX  3

$ vagcan -v dev vcds stats
? success
label cache ~/.vagcan/vcds/cache.sqlite
7 tables · 3 rows

TABLES
  label_file      0
  measurement     0
  redirect        0
  adaptation      0
  long_coding     0
  registry_row    3
  registry_level  0

REGISTRY ROWS
  FFMUX  3
read from the vcds source /nowhere/sim-vcds/Labels

```
