#!/usr/bin/env python3
"""frfscope — open a Simos18 firmware's calibration maps as graphs in a browser.

Input is an .frf, .odx, or raw .bin (calibration block); the type is resolved by
extension. With a TunerPro .xdf definition it plots named maps (heatmaps for 2D,
line charts for 1D); without one it falls back to heuristic axis detection and
plots unnamed table candidates.

This is a READ-ONLY analysis helper. It never talks to a car and never writes an
ECU — see research/stage1-frf-pipeline.md for the safety boundary. Firmware
extraction reuses the external bri3d/VW_Flash toolchain (pass its directory with
--vwflash or set VW_FLASH_DIR); raw .bin input needs neither.

Usage:
    python frfscope.py FILE.frf --xdf DEF.xdf --vwflash /path/to/VW_Flash
    python frfscope.py FILE.odx --vwflash /path/to/VW_Flash
    python frfscope.py FD_4.CAL.bin            # raw block, no VW_Flash needed
"""
import argparse
import json
import os
import re
import struct
import sys
import webbrowser
import xml.etree.ElementTree as ET
from pathlib import Path


# ── VW_Flash discovery ───────────────────────────────────────────────────────

def discover_vwflash(explicit: Path | None) -> Path | None:
    """Locate a bri3d/VW_Flash checkout without the caller naming it.

    Order: --vwflash, $VW_FLASH_DIR, a copy vendored next to this script
    (scripts/vendor/VW_Flash), then the current directory."""
    here = Path(__file__).resolve().parent
    cands = [explicit,
             Path(os.environ["VW_FLASH_DIR"]) if os.environ.get("VW_FLASH_DIR") else None,
             here / "VW_Flash", here / "vendor" / "VW_Flash",
             here.parent / "vendor" / "VW_Flash", Path.cwd() / "VW_Flash"]
    for c in cands:
        if c and (Path(c) / "VW_Flash.py").exists():
            return Path(c).resolve()
    return None


def discover_default_xdf() -> Path | None:
    """A bundled definition to fall back on when --xdf is omitted.

    Alphabetically first wins, so the preferred default must sort first —
    see defs/README.md for the current ranking and why."""
    defs = Path(__file__).resolve().parent / "defs"
    if defs.is_dir():
        xdfs = sorted(defs.glob("*.xdf"))
        return xdfs[0] if xdfs else None
    return None


def ensure_vwflash_deps(vw: Path):
    """VW_Flash needs pycryptodome. If the running interpreter lacks it but the
    vendored VW_Flash has a .venv that has it, re-exec this script under that
    interpreter so the user never has to pick a python."""
    try:
        import Crypto  # noqa: F401
        return
    except ImportError:
        pass
    # A venv python is a symlink to the base interpreter, so resolve() would make
    # it compare equal to sys.executable — compare the literal paths instead, and
    # use a sentinel so a venv that still lacks the dep can't loop forever.
    venv_py = vw / ".venv" / "bin" / "python"
    if venv_py.exists() and str(venv_py) != sys.executable and not os.environ.get("_FRFSCOPE_REEXEC"):
        os.environ["_FRFSCOPE_REEXEC"] = "1"
        os.execv(str(venv_py), [str(venv_py), *sys.argv])  # replaces this process
    sys.exit(f"VW_Flash python deps missing (pycryptodome). Run:  uv sync  in {vw}")


# ── firmware extraction ──────────────────────────────────────────────────────

def load_calibration(path: Path, vwflash: Path | None) -> bytes:
    """Return the calibration block (FD_4) bytes for an frf/odx, or the file
    itself for a raw .bin."""
    ext = path.suffix.lower()
    if ext == ".bin":
        return path.read_bytes()
    if ext not in (".frf", ".odx"):
        sys.exit(f"unknown input type {ext!r}; expected .frf, .odx or .bin")
    if not vwflash or not vwflash.is_dir():
        sys.exit(
            "extracting .frf/.odx needs the VW_Flash toolchain and none was found; "
            "vendor it at scripts/vendor/VW_Flash, pass --vwflash, or set "
            "VW_FLASH_DIR (raw .bin input needs neither)"
        )
    ensure_vwflash_deps(vwflash)
    sys.path.insert(0, str(vwflash))
    os.chdir(vwflash)  # VW_Flash reads data/frf.key by relative path
    from lib.extract_flash import extract_odx_from_frf, extract_data_from_odx
    from lib.modules import simos18

    raw = path.read_bytes()
    odx = extract_odx_from_frf(raw) if ext == ".frf" else raw
    blocks, _ = extract_data_from_odx(odx, simos18.s18_flash_info)
    cal = blocks.get("FD_4")
    if cal is None:
        sys.exit(f"no FD_4/CAL block found; blocks present: {list(blocks)}")
    return bytes(cal)


# ── XDF definition parsing ───────────────────────────────────────────────────

_MATH_OK = re.compile(r"^[0-9XxeE.+\-*/() ]+$")


def _apply_math(equation: str | None, value: float) -> float:
    """Evaluate a TunerPro axis MATH equation ('X*0.1', '(X-32768)*0.05') for one
    value. Falls back to the raw value on anything non-trivial."""
    if not equation or not _MATH_OK.match(equation):
        return value
    try:
        return eval(equation.replace("X", "x").replace("x", repr(float(value))),
                    {"__builtins__": {}}, {})
    except Exception:
        return value


def _read_scalar(buf: bytes, off: int, bits: int, signed: bool) -> float | None:
    size = bits // 8
    if off < 0 or off + size > len(buf):
        return None
    fmt = {8: "b" if signed else "B", 16: "<h" if signed else "<H",
           32: "<i" if signed else "<I"}.get(bits)
    if not fmt:
        return None
    return float(struct.unpack_from(fmt, buf, off)[0])


def _axis_info(axis_el):
    """(address|None, bits, count, equation, static_labels|None) for an XDFAXIS."""
    emb = axis_el.find("EMBEDDEDDATA")
    addr = bits = None
    if emb is not None:
        a = emb.get("mmedaddress")
        addr = int(a, 16) if a and a.startswith("0x") else (int(a) if a else None)
        b = emb.get("mmedelementsizebits")
        bits = int(b) if b else None
    ic = axis_el.find("indexcount")
    count = int(ic.text) if ic is not None and ic.text else None
    math = axis_el.find("MATH")
    equation = math.get("equation") if math is not None else None
    labels = [l.get("value") for l in axis_el.findall("LABEL")]
    labels = [x for x in labels if x is not None] or None
    return addr, bits, count, equation, labels


def _axis_values(cal, addr, bits, count, equation, labels):
    if labels:
        out = []
        for s in labels:
            try:
                out.append(_apply_math(equation, float(s)))
            except ValueError:
                out.append(s)
        return out
    if addr is not None and bits and count:
        vals = []
        for i in range(count):
            raw = _read_scalar(cal, addr + i * (bits // 8), bits, False)
            vals.append(None if raw is None else round(_apply_math(equation, raw), 4))
        return vals
    return list(range(count or 0))


def _axis_label(axis_el):
    """A short axis caption from an XDFAXIS: its <units>, else 'axis N'."""
    if axis_el is None:
        return ""
    u = axis_el.findtext("units")
    return (u or "").strip()


def classify(grid) -> tuple[str, str]:
    """Judge a decoded z-grid as ok / flat / noisy from its own shape alone.

    This gates whether the VALUES look real, independent of any physical unit —
    a wrong address reads as filler (flat) or high-entropy (noisy). It is a
    plausibility gate, NOT a correctness proof: only a definition built for this
    exact software (an O10 A2L) or hardware validation certifies a map.
    """
    vals = [v for row in grid for v in row if isinstance(v, (int, float))]
    if not vals:
        return "noisy", "no readable data"
    n = len(vals)
    rng = (max(vals) - min(vals)) or 0
    same = max(vals.count(v) for v in set(vals)) / n
    jumps = []
    for row in grid:
        nums = [v for v in row if isinstance(v, (int, float))]
        jumps += [abs(a - b) for a, b in zip(nums, nums[1:])]
    rough = (sum(jumps) / len(jumps) / rng) if jumps and rng else 0.0
    if same > 0.9:
        return "flat", f"{same*100:.0f}% one value — filler / unused / wrong address"
    if rough > 0.35:
        return "noisy", f"adjacent cells jump {rough*100:.0f}% of range — garbage-like"
    return "ok", "structured"


def parse_xdf_tables(cal: bytes, xdf_path: Path) -> list[dict]:
    root = ET.parse(xdf_path).getroot()
    cats = {c.get("index"): (c.findtext("title") or "")
            for c in root.findall("XDFCATEGORY")}
    tables = []
    for t in root.findall("XDFTABLE"):
        title = t.findtext("title") or "(untitled)"
        desc = (t.findtext("description") or "").strip()
        cat = ""
        cm = t.find("CATEGORYMEM")
        if cm is not None:
            cat = cats.get(cm.get("category"), "") or cats.get(
                str(int(cm.get("category")) - 1) if cm.get("category") else "", "")
        axes = {a.get("id"): a for a in t.findall("XDFAXIS")}
        z = axes.get("z")
        if z is None:
            continue
        zaddr, zbits, _, zeq, _ = _axis_info(z)
        if zaddr is None or not zbits:
            continue
        xa = axes.get("x")
        ya = axes.get("y")
        xvals = _axis_values(cal, *_axis_info(xa)) if xa is not None else [0]
        yvals = _axis_values(cal, *_axis_info(ya)) if ya is not None else [0]
        cols = max(1, len(xvals))
        rows = max(1, len(yvals))
        zflags = z.find("EMBEDDEDDATA")
        signed = bool(zflags is not None and (int(zflags.get("mmedtypeflags", "0"), 16) & 0x01))
        grid = []
        ok = True
        for r in range(rows):
            row = []
            for c in range(cols):
                off = zaddr + (r * cols + c) * (zbits // 8)
                raw = _read_scalar(cal, off, zbits, signed)
                if raw is None:
                    ok = False
                    row.append(None)
                else:
                    row.append(round(_apply_math(zeq, raw), 4))
            grid.append(row)
        if not ok and rows * cols > 4:
            # grid ran past the block — address does not fit this binary
            continue
        status, why = classify(grid)
        tables.append({
            "title": title, "category": cat, "desc": desc,
            "address": f"0x{zaddr:X}", "rows": rows, "cols": cols,
            "x": xvals if xa is not None else None,
            "y": yvals if ya is not None else None,
            "xlab": _axis_label(xa), "ylab": _axis_label(ya),
            "zlab": _axis_label(z),
            "z": grid, "kind": "2d" if rows > 1 and cols > 1 else "1d",
            "status": status, "why": why,
        })
    return tables


# ── heuristic table detection (no definition) ────────────────────────────────

def heuristic_tables(cal: bytes, limit: int = 60) -> list[dict]:
    """Find strictly-increasing uint16-LE runs (axis breakpoints) and plot them as
    unnamed 1D candidates. Coarse — a real definition is far better."""
    n = len(cal)
    found = []
    i = 0
    while i < n - 2:
        vals = [struct.unpack_from("<H", cal, i)[0]]
        j = i + 2
        while j < n - 2:
            v = struct.unpack_from("<H", cal, j)[0]
            if v > vals[-1] and v - vals[-1] < 0x4000:
                vals.append(v)
                j += 2
            else:
                break
        if 6 <= len(vals) <= 64:
            found.append((i, vals))
            i = j
        else:
            i += 2
    found.sort(key=lambda p: len(p[1]), reverse=True)
    return [{
        "title": f"candidate @0x{off:X} (len {len(vals)})",
        "category": "heuristic axis candidates",
        "desc": "Monotonically increasing uint16 run — likely an axis/breakpoint "
                "table. Unnamed: pass --xdf for real names and scaling.",
        "address": f"0x{off:X}", "rows": 1, "cols": len(vals),
        "x": list(range(len(vals))), "y": None,
        "xlab": "index", "ylab": "", "zlab": "raw uint16",
        "z": [vals], "kind": "1d", "status": "ok", "why": "monotonic run",
    } for off, vals in found[:limit]]


# ── HTML rendering (self-contained, no external assets) ───────────────────────

_HTML = """<!doctype html><html><head><meta charset="utf-8">
<title>frfscope — {name}</title>
<style>
 body{{font:13px/1.4 -apple-system,Segoe UI,sans-serif;margin:0;background:#111;color:#ddd}}
 header{{padding:10px 16px;background:#1b1b1b;position:sticky;top:0;z-index:5;border-bottom:1px solid #333;display:flex;align-items:center;gap:14px;flex-wrap:wrap}}
 header b{{color:#6cf}} #q{{background:#000;color:#ddd;border:1px solid #444;padding:5px 8px;width:260px;border-radius:5px}}
 .legend{{display:flex;align-items:center;gap:6px;color:#888;font-size:11px}}
 .bar{{width:120px;height:10px;border-radius:3px;background:linear-gradient(90deg,rgb(0,80,255),rgb(80,180,80),rgb(255,80,0))}}
 #grid{{display:grid;grid-template-columns:repeat(auto-fill,minmax(320px,1fr));gap:12px;padding:12px}}
 .card{{background:#1b1b1b;border:1px solid #333;border-radius:7px;padding:8px;border-left-width:4px}}
 .card.ok{{border-left-color:#4a4}} .card.flat{{border-left-color:#888}} .card.noisy{{border-left-color:#c44}}
 .card .why{{font-size:10px;margin-bottom:4px}} .card.flat .why{{color:#aaa}} .card.noisy .why{{color:#f88}}
 .seg{{display:flex;gap:2px}} .seg button{{background:#222;color:#bbb;border:1px solid #444;padding:4px 9px;cursor:pointer;font-size:11px}}
 .seg button.on{{background:#2a4;color:#000;border-color:#2a4}} .seg button:first-child{{border-radius:5px 0 0 5px}} .seg button:last-child{{border-radius:0 5px 5px 0}}
 .card h3{{margin:2px 0 4px;font-size:12px;color:#9df}} .card .m{{color:#888;font-size:11px;margin-bottom:2px}}
 .card .d{{color:#aaa;font-size:11px;margin:2px 0 6px;font-style:italic}}
 .card .ax{{color:#6a6;font-size:10px;margin-bottom:4px}}
 canvas,svg{{display:block;width:100%;background:#000;border-radius:4px;cursor:crosshair}}
 .cat{{color:#f96;font-size:11px;text-transform:uppercase;letter-spacing:.5px}}
 #tip{{position:fixed;pointer-events:none;z-index:20;background:#000;border:1px solid #6cf;
   border-radius:5px;padding:5px 8px;font-size:12px;color:#fff;display:none;max-width:260px;box-shadow:0 2px 10px #000}}
 #tip .k{{color:#6cf}}
</style></head><body>
<header><b>frfscope</b> · {name} · <span id="n"></span> shown
 <input id="q" placeholder="filter by name / category…">
 <span class="seg" id="seg">
  <button data-f="ok" class="on">values ok</button>
  <button data-f="flat">flat</button>
  <button data-f="noisy">noisy</button>
  <button data-f="all">all</button></span>
 <span class="legend">low <span class="bar"></span> high · <i style="color:#666">hover a cell for X · Y · value</i></span>
</header>
<div id="grid"></div>
<div id="tip"></div>
<script>
const TABLES={data};
const grid=document.getElementById('grid'),q=document.getElementById('q'),nEl=document.getElementById('n'),tip=document.getElementById('tip');
function color(t){{const r=Math.round(255*Math.min(1,Math.max(0,t)));return `rgb(${{r}},${{Math.round(80+100*(1-Math.abs(t-.5)*2))}},${{255-r}})`;}}
function flat(z){{let a=[];for(const row of z)for(const v of row)if(typeof v==='number')a.push(v);return a;}}
function showTip(e,html){{tip.innerHTML=html;tip.style.display='block';
 let x=e.clientX+14,y=e.clientY+14;
 if(x+tip.offsetWidth>innerWidth)x=e.clientX-tip.offsetWidth-14;
 if(y+tip.offsetHeight>innerHeight)y=e.clientY-tip.offsetHeight-14;
 tip.style.left=x+'px';tip.style.top=y+'px';}}
function hideTip(){{tip.style.display='none';}}
function axName(t,which){{const l=which==='x'?t.xlab:t.ylab;
 return l||(which==='x'?'X':'Y');}}
function draw(card,t){{
 const vals=flat(t.z);if(!vals.length){{
  card.insertAdjacentHTML('beforeend','<div class="m">(no readable data at this address)</div>');return;}}
 const mn=Math.min(...vals),mx=Math.max(...vals),sp=(mx-mn)||1;
 const zl=t.zlab?` ${{t.zlab}}`:'';
 const m=document.createElement('div');m.className='m';
 m.textContent=`${{t.rows}}×${{t.cols}} @ ${{t.address}} · min ${{mn}}${{zl}} · max ${{mx}}${{zl}}`;card.appendChild(m);
 if(t.kind==='2d'){{
  const cw=Math.max(3,Math.floor(300/t.cols)),ch=Math.max(3,Math.floor(200/t.rows));
  const cv=document.createElement('canvas');cv.width=t.cols*cw;cv.height=t.rows*ch;
  const g=cv.getContext('2d');
  for(let r=0;r<t.rows;r++)for(let c=0;c<t.cols;c++){{const v=t.z[r][c];if(typeof v!=='number')continue;
   g.fillStyle=color((v-mn)/sp);g.fillRect(c*cw,r*ch,cw,ch);}}
  cv.addEventListener('mousemove',e=>{{const b=cv.getBoundingClientRect();
   let c=Math.floor((e.clientX-b.left)/b.width*t.cols),r=Math.floor((e.clientY-b.top)/b.height*t.rows);
   c=Math.max(0,Math.min(t.cols-1,c));r=Math.max(0,Math.min(t.rows-1,r));
   const v=t.z[r][c];
   const xv=t.x?t.x[c]:c, yv=t.y?t.y[r]:r;
   showTip(e,`<span class="k">${{axName(t,'x')}}:</span> ${{xv}}<br>`+
             `<span class="k">${{axName(t,'y')}}:</span> ${{yv}}<br>`+
             `<span class="k">value:</span> ${{v}}${{zl}}`);}});
  cv.addEventListener('mouseleave',hideTip);
  card.appendChild(cv);
 }} else {{
  const row=t.z[0],W=300,H=140,pad=6;
  const pts=row.map((v,i)=>{{const x=pad+i*(W-2*pad)/Math.max(1,row.length-1);
   const y=H-pad-((v-mn)/sp)*(H-2*pad);return `${{x.toFixed(1)}},${{y.toFixed(1)}}`;}}).join(' ');
  const svg=document.createElementNS('http://www.w3.org/2000/svg','svg');
  svg.setAttribute('viewBox',`0 0 ${{W}} ${{H}}`);
  svg.innerHTML=`<polyline fill="none" stroke="#6cf" stroke-width="1.5" points="${{pts}}"/>`;
  svg.addEventListener('mousemove',e=>{{const b=svg.getBoundingClientRect();
   let i=Math.round((e.clientX-b.left)/b.width*(row.length-1));
   i=Math.max(0,Math.min(row.length-1,i));
   const xv=t.x?t.x[i]:i;
   showTip(e,`<span class="k">${{axName(t,'x')}}:</span> ${{xv}}<br>`+
             `<span class="k">value:</span> ${{row[i]}}${{zl}}`);}});
  svg.addEventListener('mouseleave',hideTip);
  card.appendChild(svg);
 }}
}}
let STATUS='ok';
function render(f){{grid.innerHTML='';hideTip();let k=0;
 for(const t of TABLES){{const hay=(t.title+' '+t.category+' '+(t.desc||'')).toLowerCase();
  if(f&&!hay.includes(f))continue;
  if(STATUS!=='all'&&(t.status||'ok')!==STATUS)continue;k++;
  const card=document.createElement('div');card.className='card '+(t.status||'ok');
  const ax=(t.xlab||t.ylab)?`<div class="ax">X: ${{t.xlab||'—'}}${{t.kind==='2d'?' · Y: '+(t.ylab||'—'):''}}</div>`:'';
  const flag=(t.status&&t.status!=='ok')?`<div class="why">⚠ ${{t.status}}: ${{t.why||''}}</div>`:'';
  card.innerHTML=`<div class="cat">${{t.category||''}}</div><h3 title="${{(t.desc||'').replace(/"/g,'&quot;')}}">${{t.title}}</h3>`+
   (t.desc?`<div class="d">${{t.desc}}</div>`:'')+flag+ax;
  draw(card,t);grid.appendChild(card);}}
 nEl.textContent=k;}}
document.getElementById('seg').addEventListener('click',e=>{{
 const b=e.target.closest('button');if(!b)return;STATUS=b.dataset.f;
 for(const x of document.querySelectorAll('#seg button'))x.classList.toggle('on',x===b);
 render(q.value.trim().toLowerCase());}});
q.addEventListener('input',()=>render(q.value.trim().toLowerCase()));
render('');
</script></body></html>"""


def render_html(name: str, tables: list[dict], out: Path):
    out.write_text(_HTML.format(name=name, n=len(tables),
                                data=json.dumps(tables)), encoding="utf-8")


# ── main ─────────────────────────────────────────────────────────────────────

def main():
    ap = argparse.ArgumentParser(description="Plot Simos18 firmware maps in a browser.")
    ap.add_argument("input", type=Path, help=".frf, .odx, or raw .bin (CAL block)")
    ap.add_argument("--xdf", type=Path, help="TunerPro .xdf definition (named maps)")
    ap.add_argument("--vwflash", type=Path, default=None,
                    help="bri3d/VW_Flash directory; auto-discovered if omitted "
                         "(scripts/vendor/VW_Flash, $VW_FLASH_DIR)")
    ap.add_argument("--out", type=Path, help="output HTML (default: alongside input)")
    ap.add_argument("--report", action="store_true",
                    help="print the list of maps with bad/untrustworthy values")
    ap.add_argument("--no-open", action="store_true", help="don't launch a browser")
    args = ap.parse_args()

    inp = args.input.resolve()
    out = (args.out or inp.with_suffix(".frfscope.html")).resolve()
    xdf = (args.xdf.resolve() if args.xdf else discover_default_xdf())  # before chdir
    vw = discover_vwflash(args.vwflash)
    if inp.suffix.lower() in (".frf", ".odx"):
        print(f"VW_Flash: {vw or 'NOT FOUND'}")
    if xdf and not args.xdf:
        print(f"definition (default): {xdf.name} — pass --xdf to override")
    cal = load_calibration(inp, vw)

    if xdf:
        tables = parse_xdf_tables(cal, xdf)
        note = f"{len(tables)} named tables from {xdf.name}"
    else:
        tables = heuristic_tables(cal)
        note = f"{len(tables)} heuristic candidates (no definition — pass --xdf for names)"
    print(f"CAL {len(cal)} bytes · {note}")

    if xdf:
        from collections import Counter
        counts = Counter(t["status"] for t in tables)
        print(f"value gate: ok={counts['ok']} flat={counts['flat']} noisy={counts['noisy']}"
              " · 'ok' = plausible, not certified (only an exact-version A2L proves a map)")
        bad = [t for t in tables if t["status"] != "ok"]
        if args.report and bad:
            print(f"\n--- {len(bad)} maps with bad/untrustworthy values (address in this binary) ---")
            for t in sorted(bad, key=lambda x: x["status"]):
                print(f"  {t['status']:5} {t['address']:>9}  {t['title'][:44]:44.44}  {t['why']}")

    render_html(inp.name, tables, out)
    print(f"wrote {out}")
    if not args.no_open:
        webbrowser.open(out.as_uri())


if __name__ == "__main__":
    main()
