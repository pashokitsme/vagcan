# Opening a shifted `.rod` by a known mask

`dev vcds rod --mask <HEX>` opens a shifted file whose eight-byte IV mask is already known,
instead of searching for its key. The mask is 16 hex digits and is checked before the file is
read, so a malformed one is a plain error.

```console
$ vagcan dev vcds rod EV_Whatever.rod --mask nothex
? failed
error: the mask must be 16 hex digits (8 bytes), got "nothex"

```

It cannot be combined with the key search it replaces.

```console
$ vagcan dev vcds rod EV_Whatever.rod --mask 0011223344556677 --no-crack
? failed
error: the argument '--mask <HEX>' cannot be used with '--no-crack'

Usage: vagcan dev vcds rod --mask <HEX> <FILE>

For more information, try '--help'.

```
