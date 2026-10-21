# `setup --list` over two sources, at 80 columns

The VCDS store's path is too long to sit beside its row at 80 columns, so it goes on a line
of its own under the row. Both rows stay, and at `-v` each row's details sit under that row.

```console
$ VAGCAN_COLUMNS=80 vagcan setup --list
? success
2 sources on this machine

   ID  KIND  PROJECT  LANG  READ        CHANNELS  PATH
✗  v1  vcds  -        eng   2026-10-21         0
   /nowhere/a-long-way-down/to-a-vcds-installation/Labels
✗  p1  odis  SIM00    eng   2026-10-21         0  /nowhere/SIM00-odis

$ VAGCAN_COLUMNS=80 vagcan -v setup --list
? success
2 sources on this machine

   ID  KIND  PROJECT  LANG  READ        CHANNELS  PATH
✗  v1  vcds  -        eng   2026-10-21         0
   /nowhere/a-long-way-down/to-a-vcds-installation/Labels
    ~/.vagcan/vcds/cache.sqlite · its folder is gone; its rows still serve
✗  p1  odis  SIM00    eng   2026-10-21         0  /nowhere/SIM00-odis
    ~/.vagcan/data/SIM00/cache.sqlite · its folder is gone; its rows still serve

```
