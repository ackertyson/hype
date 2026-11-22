#!/usr/bin/env python3
"""Generate hype's Recursive glyph data: outlines as SVG path data, and each
glyph's ink coverage over a cell's 2x4 subpixels, measured from those exact
outlines. Also prints a report comparing weights and cell geometries.

Offline and rarely run; hype's build does not need it. Setup:

    python3 -m venv build/.venv
    build/.venv/bin/pip install --require-hashes --no-deps -r build/requirements.txt
    build/.venv/bin/python -I build/glyphs.py

The font release is downloaded once into build/fonts/ (gitignored) and must
match the pinned SHA-256.

Coverage is sampled with the nonzero winding rule rather than computed as
exact clipped areas, because variable-font instances keep overlapping
contours, which exact area clipping would count twice.
"""

import argparse
import hashlib
import io
import pathlib
import re
import sys
import urllib.request
import zipfile

from fontTools.pens.basePen import BasePen
from fontTools.pens.recordingPen import DecomposingRecordingPen
from fontTools.pens.roundingPen import RoundingPen
from fontTools.pens.teePen import TeePen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont

ROOT = pathlib.Path(__file__).resolve().parent.parent
VERSION = "1.085"
ZIP_NAME = f"ArrowType-Recursive-{VERSION}.zip"
ZIP_URL = f"https://github.com/arrowtype/recursive/releases/download/v{VERSION}/{ZIP_NAME}"
# GitHub publishes no checksum for this release; pinned from the first
# download (2026-11-22).
ZIP_SHA256 = "cbcbdf7a0e181d284a9235e09ed5f3873e527bc5dd1d977df71cdc1ff937da02"
TTF_IN_ZIP = f"ArrowType-Recursive-{VERSION}/Recursive_Desktop/Recursive_VF_{VERSION}.ttf"

# Monospace, linear (not casual), upright; CRSV 0.5 is the font's "auto"
# (cursive forms only when slanted).
AXES = {"MONO": 1, "CASL": 0, "slnt": 0, "CRSV": 0.5}
# Italic (for --text's _italic_ markup): the full slant, where CRSV's "auto"
# switches in Recursive's cursive letterforms.
ITALIC_AXES = {"slnt": -15}
REPORT_WEIGHTS = [300, 400, 500, 600, 700, 800, 875, 900, 1000]
# Regular for light tones up to the heaviest for the most ink in dark ones
# (on paper), with weights between so neighboring cells change weight in
# small steps: a single 400 -> 1000 jump looked jarring. The axis is
# non-linear (ink grows slowly to 800, jumps by 900, then slows), so these
# are chosen for even steps in an average lowercase letter's ink in the fill
# geometry: 23.4%, 29.4%, 35.7%, ~42.9%, 49.3%.
EMIT_WEIGHTS = [400, 600, 800, 875, 1000]

# The cell in font units, from the font's own metrics: the 600-unit advance,
# and ascender 950 to descender -250. That is exactly 1:2, the shape hype
# assumes. Outlines are emitted cell-local: origin at the top-left, y down.
CELL_W, CELL_H, ASCENDER = 600, 1200, 950
# Geometries map a box within the cell onto the cell: (x, y, w, h), cell-local.
GEOMETRIES = {
    # As the font sets type: the whole line height.
    "natural": (0, 0, CELL_W, CELL_H),
    # Glyphs enlarged to fill the cell: from the descender of p/q/y (-200) to
    # the top of | { } (760), and x 30..570 (W's ink reaches 23..575). Shown
    # as a 1:2 cell this is a 1.125x vertical stretch, and the few glyphs
    # reaching beyond (g, $) overlap their neighbors slightly.
    "fill": (30, ASCENDER - 760, 540, 960),
}
# Path data units per font unit: outlines are whole font units, but implied
# curve midpoints fall on halves, so doubling makes every number whole
# (measured 9% smaller path data). The SVG transforms scale back.
PATH_UNITS = 2
SAMPLES = 32  # per subpixel side
FLATTEN_STEPS = 16  # line segments per curve when sampling


class FlattenPen(BasePen):
    """Collects an outline as straight edges, curves split into segments."""

    def __init__(self):
        super().__init__(None)
        self.edges = []
        self.cur = self.start = None

    def _moveTo(self, p):
        self.cur = self.start = p

    def _lineTo(self, p):
        self.edges.append((self.cur, p))
        self.cur = p

    def _qCurveToOne(self, p1, p2):
        p0 = self.cur
        for i in range(1, FLATTEN_STEPS + 1):
            t = i / FLATTEN_STEPS
            a, b, c = (1 - t) ** 2, 2 * (1 - t) * t, t * t
            self._lineTo((a * p0[0] + b * p1[0] + c * p2[0], a * p0[1] + b * p1[1] + c * p2[1]))

    def _curveToOne(self, p1, p2, p3):
        p0 = self.cur
        for i in range(1, FLATTEN_STEPS + 1):
            t = i / FLATTEN_STEPS
            a, b, c, d = (1 - t) ** 3, 3 * (1 - t) ** 2 * t, 3 * (1 - t) * t * t, t**3
            self._lineTo(
                (
                    a * p0[0] + b * p1[0] + c * p2[0] + d * p3[0],
                    a * p0[1] + b * p1[1] + c * p2[1] + d * p3[1],
                )
            )

    def _closePath(self):
        if self.cur != self.start:
            self._lineTo(self.start)

    _endPath = _closePath  # fonts fill open contours as closed


def fmt_num(v):
    """Shortest SVG form of a whole or half font unit: 12, -3, .5, -1.5."""
    if v == int(v):
        return str(int(v))
    s = repr(float(v))
    return s.replace("0.", ".", 1) if s.startswith(("0.", "-0.")) else s


class RelPathPen(BasePen):
    """SVG path data in its compact form: relative commands (small deltas
    rather than absolute positions), h/v for axis-aligned lines, repeated
    command letters and needless separators left out. Also records each
    command's absolute points, to check the encoding round-trips."""

    def __init__(self):
        super().__init__(None)
        self.d = ""
        self.points = []  # (command, [absolute points])
        self.cur = self.start = self.ctrl = (0, 0)
        self.last = None

    def _emit(self, cmd, deltas, points):
        if cmd != self.last or cmd in "mz":
            self.d += cmd
        for v in deltas:
            t = fmt_num(v * PATH_UNITS)
            if self.d and not self.d[-1].isalpha() and not t.startswith("-"):
                self.d += " "
            self.d += t
        self.last = cmd
        # Recorded as the decoder reports it: a t as the q it stands for.
        self.points.append(("q" if cmd == "t" else cmd, points))

    def _rel(self, p):
        return (p[0] - self.cur[0], p[1] - self.cur[1])

    def _moveTo(self, p):
        self._emit("m", self._rel(p), [p])
        self.cur = self.start = p

    def _lineTo(self, p):
        dx, dy = self._rel(p)
        if dy == 0:
            self._emit("h", (dx,), [p])
        elif dx == 0:
            self._emit("v", (dy,), [p])
        else:
            self._emit("l", (dx, dy), [p])
        self.cur = p

    def _qCurveToOne(self, p1, p2):
        # TrueType chains curves through implied on-curve points midway
        # between control points, and SVG's t takes its control point as the
        # previous one reflected through the current point: for such a chain
        # that is exactly this curve's control point, so it can be left out.
        reflected = (2 * self.cur[0] - self.ctrl[0], 2 * self.cur[1] - self.ctrl[1])
        if self.last in ("q", "t") and reflected == tuple(p1):
            self._emit("t", self._rel(p2), [p1, p2])
        else:
            self._emit("q", (*self._rel(p1), *self._rel(p2)), [p1, p2])
        self.cur, self.ctrl = p2, p1

    def _curveToOne(self, p1, p2, p3):
        self._emit("c", (*self._rel(p1), *self._rel(p2), *self._rel(p3)), [p1, p2, p3])
        self.cur = p3

    def _closePath(self):
        self._emit("z", (), [])
        # After z, SVG continues from the start of the closed subpath.
        self.cur = self.start

    _endPath = _closePath  # fonts fill open contours as closed


def decode_path(d):
    """Absolute points per command of relative SVG path data, by the SVG
    grammar: an omitted command letter repeats the last (l after m), and t is
    a q whose control point is the previous q or t's reflected through the
    current point (or the current point itself after anything else)."""
    tokens = re.findall(r"[a-zA-Z]|-?(?:\d+\.?\d*|\.\d+)", d)
    arity = {"m": 2, "l": 2, "h": 1, "v": 1, "q": 4, "t": 2, "c": 6, "z": 0}
    out, cur, start, ctrl, cmd, prev, i = [], (0, 0), (0, 0), None, None, None, 0
    while i < len(tokens):
        if tokens[i].isalpha():
            cmd, i = tokens[i], i + 1
        elif cmd == "m":
            cmd = "l"
        n = [float(t) / PATH_UNITS for t in tokens[i : i + arity[cmd]]]
        i += arity[cmd]
        if cmd == "z":
            out.append(("z", []))
            cur, prev = start, cmd
            continue
        if cmd == "h":
            pts = [(cur[0] + n[0], cur[1])]
        elif cmd == "v":
            pts = [(cur[0], cur[1] + n[0])]
        elif cmd == "t":
            c = (2 * cur[0] - ctrl[0], 2 * cur[1] - ctrl[1]) if prev in ("q", "t") else cur
            pts = [c, (cur[0] + n[0], cur[1] + n[1])]
        else:
            pts = [(cur[0] + n[k], cur[1] + n[k + 1]) for k in range(0, len(n), 2)]
        out.append(("q" if cmd == "t" else cmd, pts))
        if cmd in ("q", "t"):
            ctrl = pts[0]
        cur, prev = pts[-1], cmd
        if cmd == "m":
            start = cur
    return out


def coverage(edges, box):
    """Fraction of each 2x4 subpixel of `box` inside the outline (nonzero rule)."""
    x0, y0, w, h = box
    rows, cols = 4 * SAMPLES, 2 * SAMPLES
    dy, dx = h / rows, w / cols
    # Bucket edges by the sample rows they cross.
    by_row = [[] for _ in range(rows)]
    for (ax, ay), (bx, by) in edges:
        if ay == by:
            continue
        lo, hi = min(ay, by), max(ay, by)
        r0 = max(0, int((lo - y0) / dy - 0.5) - 1)
        r1 = min(rows - 1, int((hi - y0) / dy - 0.5) + 1)
        for r in range(r0, r1 + 1):
            by_row[r].append((ax, ay, bx, by))
    inside = [0] * 8
    for r in range(rows):
        y = y0 + (r + 0.5) * dy
        xs = []
        for ax, ay, bx, by in by_row[r]:
            if (ay <= y < by) or (by <= y < ay):
                xs.append((ax + (y - ay) * (bx - ax) / (by - ay), 1 if by > ay else -1))
        if not xs:
            continue
        xs.sort()
        wind, k = 0, 0
        for c in range(cols):
            x = x0 + (c + 0.5) * dx
            while k < len(xs) and xs[k][0] < x:
                wind += xs[k][1]
                k += 1
            if wind:
                inside[(r // SAMPLES) * 2 + c // SAMPLES] += 1
    return [n / (SAMPLES * SAMPLES) for n in inside]


def selftest():
    """The sampler on shapes with known coverage."""

    def rect(x0, y0, x1, y1, ccw=False):
        pts = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
        if ccw:
            pts.reverse()
        return [(pts[i], pts[(i + 1) % 4]) for i in range(4)]

    box = GEOMETRIES["natural"]
    assert coverage(rect(0, 0, CELL_W, CELL_H), box) == [1.0] * 8, "full cell"
    assert coverage([], box) == [0.0] * 8, "empty cell"
    # Left half: the left subpixel of every row.
    assert coverage(rect(0, 0, CELL_W / 2, CELL_H), box) == [1.0, 0.0] * 4, "left half"
    # Overlapping contours count once (nonzero), and an opposite-winding
    # contour inside punches a hole.
    two = rect(0, 0, CELL_W, CELL_H) + rect(0, 0, CELL_W, CELL_H / 2)
    assert coverage(two, box) == [1.0] * 8, "overlap"
    hole = rect(0, 0, CELL_W, CELL_H) + rect(0, 0, CELL_W, CELL_H / 4, ccw=True)
    assert coverage(hole, box) == [0.0, 0.0] + [1.0] * 6, "hole"


def load_font():
    path = ROOT / "build" / "fonts" / ZIP_NAME
    if not path.exists():
        path.parent.mkdir(parents=True, exist_ok=True)
        print(f"downloading {ZIP_URL}", file=sys.stderr)
        tmp = path.with_suffix(".part")
        urllib.request.urlretrieve(ZIP_URL, tmp)
        tmp.rename(path)
    data = path.read_bytes()
    digest = hashlib.sha256(data).hexdigest()
    if digest != ZIP_SHA256:
        sys.exit(f"{path}: SHA-256 {digest} does not match pinned {ZIP_SHA256}")
    with zipfile.ZipFile(io.BytesIO(data)) as z:
        return TTFont(io.BytesIO(z.read(TTF_IN_ZIP)))


def measure(vf, weight, italic=False):
    """{char: (svg path, edges)} for printable ASCII at `weight`."""
    font = instantiateVariableFont(vf, dict(AXES, wght=weight, **(ITALIC_AXES if italic else {})))
    cmap, glyphs = font.getBestCmap(), font.getGlyphSet()
    out = {}
    for code in range(32, 127):
        # Points are whole font units; implied curve midpoints may be halves.
        svg, flat = RelPathPen(), FlattenPen()
        # Flip to cell-local y-down, then round to whole font units, so the
        # emitted paths and the measured outlines are identical.
        pen = TransformPen(RoundingPen(TeePen(svg, flat)), (1, 0, 0, -1, 0, ASCENDER))
        # Composite glyphs are drawn from their components' outlines.
        rec = DecomposingRecordingPen(glyphs)
        glyphs[cmap[code]].draw(rec)
        rec.replay(pen)
        # The compact path data must decode to exactly the measured points.
        if decode_path(svg.d) != svg.points:
            sys.exit(f"path encoding does not round-trip for {chr(code)!r} at {weight}")
        out[chr(code)] = (svg.d, flat.edges)
    return out


def current_table():
    """The GLYPHS table in src/render.rs: [(char, [8 coverages])], in order."""
    src = (ROOT / "src" / "render.rs").read_text()
    body = re.search(r"const GLYPHS: \[\(u8, \[u8; 8\]\); \d+\] = \[(.*?)\n\];", src, re.S)
    rows = re.findall(r"\(b'(\\?.)', \[([\d, ]+)\]\)", body.group(1))
    return [(c[-1], [int(v) for v in vals.split(",")]) for c, vals in rows]


def pearson(a, b):
    n = len(a)
    ma, mb = sum(a) / n, sum(b) / n
    cov = sum((x - ma) * (y - mb) for x, y in zip(a, b))
    va = sum((x - ma) ** 2 for x in a) ** 0.5
    vb = sum((y - mb) ** 2 for y in b) ** 0.5
    return cov / (va * vb) if va and vb else float("nan")


def mean(v):
    return sum(v) / len(v)


def report(cov, ramp, menlo):
    """cov[(geometry, weight)][char] = 8 raw fractions."""
    print("Ink coverage of a cell (raw fraction of its area):")
    print(f"  {'geometry':9} {'wght':>4}  {'densest glyph':>15}  {'lowercase avg':>13}  {'shape spread':>12}")
    for (geo, wght), table in sorted(cov.items()):
        densest = max(ramp, key=lambda c: mean(table[c]))
        lower = mean([mean(table[c]) for c in ramp if c.islower()])
        # Shape distinctness: mean distance from each ramp glyph to its
        # nearest other ramp glyph.
        nn = []
        for c in ramp:
            nn.append(
                min(
                    sum((x - y) ** 2 for x, y in zip(table[c], table[d])) ** 0.5
                    for d in ramp
                    if d != c
                )
            )
        print(
            f"  {geo:9} {wght:>4}  {densest!r:>5} {mean(table[densest]):6.1%}   "
            f"{lower:12.1%}   {mean(nn):12.3f}"
        )
    print()
    print("Interpolating coverage between weights (max error, raw fraction):")
    for geo in GEOMETRIES:
        for lo, mid, hi in [(300, 400, 500), (400, 600, 800), (600, 800, 1000), (400, 700, 1000)]:
            err = max(
                abs(cov[geo, mid][c][i] - (cov[geo, lo][c][i] * (hi - mid) + cov[geo, hi][c][i] * (mid - lo)) / (hi - lo))
                for c in ramp
                for i in range(8)
            )
            print(f"  {geo:9} {mid} from {lo} and {hi}: {err:.3f}")
    print()
    t = cov["natural", 400]
    r = sorted((pearson(t[c], m), c) for c, m in menlo if any(m) and any(t[c]))
    print("Sanity check, shape correlation with the current Menlo/SF Mono/Monaco table:")
    print(f"  median {r[len(r) // 2][0]:.2f}; lowest: " + ", ".join(f"{c!r} {p:.2f}" for p, c in r[:5]))


def byte_lit(c):
    return "b'" + ("\\'" if c == "'" else "\\\\" if c == "\\" else c) + "'"


def rust_table(name, entries, scale, doc):
    """entries: [(char, weight index, 8 raw fractions)]."""
    lines = [f"/// {doc}", f"pub const {name}: [((u8, u8), [u8; 8]); {len(entries)}] = ["]
    for c, wi, fracs in entries:
        vals = ", ".join(str(min(255, round(v * scale))) for v in fracs)
        lines.append(f"    (({byte_lit(c)}, {wi}), [{vals}]),")
    lines.append("];")
    return "\n".join(lines)


def rust_paths(name, doc, outlines):
    """outlines: {weight: {char: (path, edges)}}, as from `measure`."""
    drawn = [
        (c, wi, outlines[w][c][0])
        for wi, w in enumerate(EMIT_WEIGHTS)
        for c in map(chr, range(32, 127))
        if outlines[w][c][0]
    ]
    out = [*doc, f"pub const {name}: [((u8, u8), &str); {len(drawn)}] = ["]
    for c, wi, d in drawn:
        # As rustfmt lays out these long entries, so it leaves the file be.
        out += ["    (", f"        ({byte_lit(c)}, {wi}),", f'        "{d}",', "    ),"]
    return out + ["];", ""]


def emit(path, outlines, cov, italic, italic_cov, ramp):
    other = [chr(c) for c in range(32, 127) if chr(c) not in ramp]
    out = [
        "//! @generated by build/glyphs.py: do not edit.",
        f"//! Glyph outlines and coverage from the Recursive font, version {VERSION}",
        f"//! (MONO {AXES['MONO']}, CASL {AXES['CASL']}, slnt {AXES['slnt']}, CRSV {AXES['CRSV']}).",
        "//! Copyright 2020 The Recursive Project Authors",
        "//! (https://github.com/arrowtype/recursive). The outline data is licensed",
        "//! under the SIL Open Font License, Version 1.1: see LICENSE-Recursive.txt.",
        "",
        f"// The cell in font units is {CELL_W}x{CELL_H}: the advance width by the line height.",
        "",
    ]
    for geo, (bx, by, bw, bh) in GEOMETRIES.items():
        # SVG transform mapping the geometry's box onto a 1x2-unit cell.
        sx, sy = 1 / bw, 2 / bh
        # Path data is in PATH_UNITS per font unit; the translation is not.
        tx, ty = -bx * sx, -by * sy
        sx, sy = sx / PATH_UNITS, sy / PATH_UNITS
        decl = f"pub const {geo.upper()}_TRANSFORM: &str ="
        value = f'"matrix({sx!r} 0 0 {sy!r} {tx!r} {ty!r})";'
        # rustfmt moves a value that overflows 100 columns to its own line,
        # if it fits there.
        if len(decl) + 1 + len(value) > 100 and 4 + len(value) <= 100:
            lines = [decl, "    " + value]
        else:
            lines = [decl + " " + value]
        out += [
            f"/// Maps the {geo} geometry's box (x {bx}, y {by}, {bw}x{bh} font units,",
            f"/// cell-local; path data is in {PATH_UNITS} units per font unit) onto a",
            "/// 1x2-unit cell, as an SVG transform.",
            *lines,
            "",
        ]
    # Glyphs are labeled (char, weight index), the index into WEIGHTS.
    weights = ", ".join(map(str, EMIT_WEIGHTS))
    out += [
        "/// The font weights drawn; glyphs are labeled (char, index into this).",
        f"pub const WEIGHTS: [u16; {len(EMIT_WEIGHTS)}] = [{weights}];",
        "",
    ]
    out += rust_paths(
        "PATHS",
        [
            "/// Outlines as relative SVG path data, cell-local (origin top-left, y",
            f"/// down, {PATH_UNITS} units per font unit). Space has none.",
        ],
        outlines,
    )
    out += rust_paths(
        "ITALIC_PATHS",
        [f"/// Italic outlines (slnt {ITALIC_AXES['slnt']}: cursive forms), as `PATHS`."],
        italic,
    )
    printable = [chr(c) for c in range(32, 127)]
    for geo in GEOMETRIES:
        # One scale per geometry, shared by all weights and styles so heavier
        # glyphs keep their extra ink: the densest subpixel reads as 255.
        scale = 255 / max(
            v
            for w in EMIT_WEIGHTS
            for t in (cov[geo, w], italic_cov[geo, w])
            for c in printable
            for v in t[c]
        )
        # One ramp of every weight's glyphs, so the density ramp and shape
        # matching choose the weight along with the glyph. A blank is a
        # blank at any weight: just one space.
        merged = [
            (c, wi, cov[geo, w][c])
            for wi, w in enumerate(EMIT_WEIGHTS)
            for c in ramp
            if c != " " or wi == 0
        ]
        merged.sort(key=lambda e: (mean(e[2]), e[0], e[1]))
        others = [(c, wi, cov[geo, w][c]) for wi, w in enumerate(EMIT_WEIGHTS) for c in other]
        g = geo.upper()
        out.append(
            rust_table(
                f"RAMP_{g}",
                merged,
                scale,
                f"The ASCII ramp of all weights, by mean coverage, in the {geo} geometry.",
            )
        )
        out.append("")
        out.append(
            rust_table(
                f"OTHER_{g}",
                others,
                scale,
                "The other printable ASCII glyphs at each weight, same scale (for --text).",
            )
        )
        out.append("")
        italics = [
            (c, wi, italic_cov[geo, w][c])
            for wi, w in enumerate(EMIT_WEIGHTS)
            for c in printable
            if c != " "
        ]
        out.append(
            rust_table(
                f"ITALIC_{g}",
                italics,
                scale,
                f"Every printable italic glyph at each weight, same scale, in the {geo} geometry.",
            )
        )
        out.append("")
    path.write_text("\n".join(out))
    print(f"\nwrote {path.relative_to(ROOT)} ({path.stat().st_size} bytes)")


def proof(path, outlines, wght):
    """An SVG sheet of every glyph in its cell (gray) with the fill box (red),
    to check the extracted outlines by eye."""
    cols, pad = 14, 100  # about square
    chars = [chr(c) for c in range(32, 127)]
    rows = -(-len(chars) // cols)
    fx, fy, fw, fh = GEOMETRIES["fill"]
    w, h = cols * (CELL_W + pad) + pad, rows * (CELL_H + pad) + pad
    out = [
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w // 5}" height="{h // 5}">',
        f'<rect width="{w}" height="{h}" fill="#fff"/>',
    ]
    for i, c in enumerate(chars):
        x, y = pad + i % cols * (CELL_W + pad), pad + i // cols * (CELL_H + pad)
        out.append(f'<g transform="translate({x} {y})">')
        out.append(f'<rect width="{CELL_W}" height="{CELL_H}" fill="none" stroke="#bbb" stroke-width="4"/>')
        out.append(f'<rect x="{fx}" y="{fy}" width="{fw}" height="{fh}" fill="none" stroke="#e33" stroke-width="3"/>')
        d = outlines[wght][c][0]
        out.append(f'<path transform="scale({1 / PATH_UNITS})" d="{d}" fill="#000"/></g>')
    out.append("</svg>")
    path.write_text("\n".join(out))
    print(f"wrote {path}")


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--out", default=str(ROOT / "src" / "recursive.rs"))
    ap.add_argument("--report-only", action="store_true")
    ap.add_argument("--proof", metavar="SVG", help="also write a proof sheet of the outlines")
    ap.add_argument("--proof-weight", type=int, default=EMIT_WEIGHTS[0])
    ap.add_argument("--proof-italic", action="store_true")
    args = ap.parse_args()

    selftest()
    vf = load_font()
    menlo = current_table()
    ramp = [c for c, _ in menlo]
    outlines, cov, italic, italic_cov = {}, {}, {}, {}
    for wght in sorted(set(REPORT_WEIGHTS) | set(EMIT_WEIGHTS)):
        outlines[wght] = measure(vf, wght)
        for geo, box in GEOMETRIES.items():
            cov[geo, wght] = {c: coverage(e, box) for c, (_, e) in outlines[wght].items()}
    for wght in EMIT_WEIGHTS:
        italic[wght] = measure(vf, wght, italic=True)
        for geo, box in GEOMETRIES.items():
            italic_cov[geo, wght] = {c: coverage(e, box) for c, (_, e) in italic[wght].items()}
    report(cov, ramp, menlo)
    print("\nItalic, average lowercase ink (fill):", end="")
    for w in EMIT_WEIGHTS:
        t = italic_cov["fill", w]
        print(f"  {w} {mean([mean(t[c]) for c in ramp if c.islower()]):.1%}", end="")
    print()
    if not args.report_only:
        emit(pathlib.Path(args.out), outlines, cov, italic, italic_cov, ramp)
    if args.proof:
        source = italic if args.proof_italic else outlines
        if args.proof_weight not in source:
            source[args.proof_weight] = measure(vf, args.proof_weight, args.proof_italic)
        proof(pathlib.Path(args.proof), source, args.proof_weight)


if __name__ == "__main__":
    main()
