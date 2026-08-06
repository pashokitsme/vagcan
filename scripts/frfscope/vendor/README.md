# scripts/vendor

Local checkouts of external tools that `scripts/` depends on. Nothing here is
committed (see `.gitignore`) — each is cloned on the machine that needs it.

## VW_Flash

`bri3d/VW_Flash` — used **read-only** by `../frfscope` to decrypt/extract Simos18
firmware containers (`.frf`/`.odx`) into calibration binaries. frfscope calls
only its extraction path; it never flashes. See `../../SAFETY.md`.

Install / reinstall:

```bash
git clone --depth 1 https://github.com/bri3d/VW_Flash scripts/vendor/VW_Flash
cd scripts/vendor/VW_Flash && uv sync      # pulls pycryptodome etc.
```

frfscope auto-discovers this path. The LZSS C build (`lib/lzss`) is **not**
required — it is only used for flashing (compression); extraction decompresses
in pure Python.
