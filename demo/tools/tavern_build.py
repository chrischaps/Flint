#!/usr/bin/env python3
"""Builds the Rusty Flint Tavern's models and textures (#295).

Everything the two tavern scenes draw (demo/phase4_runtime.scene.toml and
demo/showcase_tavern.scene.toml) is made here, from code, so an agent can
change a table's height or a window's width and rebuild:

    python demo/tools/tavern_build.py --textures   # flint gen the five specs, then models
    python demo/tools/tavern_build.py              # models only (textures already built)

Textures: demo/specs/tavern_{floor,timber,fieldstone,plaster,iron}.procgen.toml
go through `flint gen` (target/release/flint, or --flint PATH) and land as
JPEGs in demo/textures/tavern/: <name>_color.jpg, <name>_normal.jpg and
<name>_mr.jpg (glTF metallic-roughness: G roughness, B metal).

Models: demo/models/tavern_*.glb, binary glTF whose images are those JPEGs by
relative URI (../textures/tavern/...), so seven models share one copy of each.
Every model is in metres with +Y up; the room's own pieces (shell, hearth) are
in the main hall's coordinates, so their entities sit at the origin, and each
prop's origin is where it stands (a door's is its hinge, a hanging light's its
hook). UVs are world-scaled, so a texture keeps its size on every board.

Only the standard library and Pillow (for the JPEGs).
"""

from __future__ import annotations

import argparse
import json
import math
import random
import shutil
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

DEMO = Path(__file__).resolve().parent.parent
REPO = DEMO.parent
TEX_DIR = DEMO / "textures" / "tavern"
MODEL_DIR = DEMO / "models"
TEX_URI = "../textures/tavern/"

# ─── Materials ──────────────────────────────────────────────────────────────
#
# tex: which texture set (tavern_<tex>_*), tile: metres per texture repeat,
# tint: baseColorFactor, metal/rough: factors (with a texture they scale its
# channels), emit: emissive colour, strength: KHR_materials_emissive_strength.

TEXTURE_SETS = ["floor", "plank", "timber", "fieldstone", "plaster", "iron"]
METAL_SETS = {"iron"}

MATS = {
    "oak":      dict(tex="floor", tile=2.4),
    "board":    dict(tex="plank", tile=1.8),
    "board_dark": dict(tex="plank", tile=1.8, tint=(0.62, 0.52, 0.46)),
    "stave":    dict(tex="plank", tile=1.0),
    "shingle":  dict(tex="floor", tile=1.2, tint=(0.42, 0.4, 0.42)),
    "timber":   dict(tex="timber", tile=1.6),
    "bark":     dict(tex="timber", tile=0.6, tint=(0.62, 0.55, 0.5)),
    "stone":    dict(tex="fieldstone", tile=1.8),
    "flag":     dict(tex="fieldstone", tile=1.1, tint=(0.8, 0.78, 0.76)),
    "coldstone": dict(tex="fieldstone", tile=1.8, tint=(0.86, 0.88, 0.95)),
    "soot":     dict(tex="fieldstone", tile=1.0, tint=(0.06, 0.05, 0.045), rough=1.0),
    "plaster":  dict(tex="plaster", tile=2.6, tint=(0.9, 0.86, 0.8)),
    "plaster_smoked": dict(tex="plaster", tile=2.6, tint=(0.5, 0.44, 0.38)),
    "iron":     dict(tex="iron", tile=0.5, metal=1.0, rough=1.0),
    "brass":    dict(color=(0.78, 0.55, 0.26), metal=1.0, rough=0.32),
    "pewter":   dict(color=(0.6, 0.6, 0.58), metal=1.0, rough=0.38),
    "wax":      dict(color=(0.92, 0.84, 0.66), rough=0.55),
    "wick":     dict(color=(0.05, 0.04, 0.03), rough=0.9),
    "flame":    dict(color=(1.0, 0.7, 0.3), emit=(1.0, 0.62, 0.22), strength=14.0),
    "ember":    dict(color=(0.3, 0.08, 0.02), emit=(1.0, 0.3, 0.05), strength=5.0),
    "lantern_glass": dict(color=(1.0, 0.82, 0.55), emit=(1.0, 0.7, 0.35), strength=1.6, rough=0.2),
    "glass_green": dict(color=(0.08, 0.22, 0.1), rough=0.06),
    "glass_amber": dict(color=(0.42, 0.18, 0.04), rough=0.06),
    "glass_dark":  dict(color=(0.06, 0.04, 0.035), rough=0.05),
    "glass_clear": dict(color=(0.55, 0.6, 0.55), rough=0.04),
    "crock":    dict(color=(0.55, 0.4, 0.26), rough=0.35),
    "crock_dark": dict(color=(0.24, 0.16, 0.1), rough=0.3),
    "cork":     dict(color=(0.5, 0.36, 0.22), rough=0.9),
    "ale":      dict(color=(0.86, 0.76, 0.56), rough=0.85),
}


# ─── Little linear algebra ──────────────────────────────────────────────────

def add(a, b): return (a[0] + b[0], a[1] + b[1], a[2] + b[2])
def sub(a, b): return (a[0] - b[0], a[1] - b[1], a[2] - b[2])
def scl(a, s): return (a[0] * s, a[1] * s, a[2] * s)
def dot(a, b): return a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
def cross(a, b): return (a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0])
def norm(a):
    l = math.sqrt(dot(a, a))
    return (a[0] / l, a[1] / l, a[2] / l) if l > 1e-12 else (0.0, 1.0, 0.0)


class Xf:
    """A rigid transform: columns of a rotation plus a translation."""

    def __init__(self, cols=((1, 0, 0), (0, 1, 0), (0, 0, 1)), t=(0, 0, 0)):
        self.c = cols
        self.t = t

    def rot(self, v):
        c = self.c
        return (c[0][0] * v[0] + c[1][0] * v[1] + c[2][0] * v[2],
                c[0][1] * v[0] + c[1][1] * v[1] + c[2][1] * v[2],
                c[0][2] * v[0] + c[1][2] * v[1] + c[2][2] * v[2])

    def pt(self, v):
        return add(self.rot(v), self.t)

    def then(self, outer: "Xf") -> "Xf":
        """This transform followed by `outer`."""
        return Xf(tuple(outer.rot(col) for col in self.c), outer.pt(self.t))


def rot_axis(axis, deg, t=(0, 0, 0)):
    a = math.radians(deg)
    c, s = math.cos(a), math.sin(a)
    if axis == "x":
        cols = ((1, 0, 0), (0, c, s), (0, -s, c))
    elif axis == "y":
        cols = ((c, 0, -s), (0, 1, 0), (s, 0, c))
    else:
        cols = ((c, s, 0), (-s, c, 0), (0, 0, 1))
    return Xf(cols, t)


def move(t):
    return Xf(t=t)


def y_to(d, t=(0, 0, 0)):
    """A rotation taking +Y to direction d."""
    y = norm(d)
    ref = (1, 0, 0) if abs(y[0]) < 0.9 else (0, 0, 1)
    x = norm(cross(y, ref))
    z = cross(x, y)
    return Xf((x, y, z), t)


# ─── Mesh building ──────────────────────────────────────────────────────────

class Mesh:
    def __init__(self):
        self.parts: dict[str, tuple[list, list, list, list]] = {}

    def part(self, mat):
        if mat not in MATS:
            raise KeyError(mat)
        return self.parts.setdefault(mat, ([], [], [], []))

    def poly(self, mat, pts, normals, uvs, want):
        """A convex polygon, wound counter-clockwise about `want`."""
        if len(pts) >= 4:
            face = cross(sub(pts[2], pts[0]), sub(pts[3], pts[1]))
        else:
            face = cross(sub(pts[1], pts[0]), sub(pts[2], pts[0]))
        if dot(face, want) < 0:
            pts, normals, uvs = pts[::-1], normals[::-1], uvs[::-1]
        P, N, U, I = self.part(mat)
        base = len(P)
        P.extend(pts)
        N.extend(normals)
        U.extend(uvs)
        for k in range(1, len(pts) - 1):
            I.extend((base, base + k, base + k + 1))

    def merge(self, other: "Mesh", xf: Xf | None = None):
        for mat, (P, N, U, I) in other.parts.items():
            p, n, u, i = self.part(mat)
            base = len(p)
            if xf:
                p.extend(xf.pt(v) for v in P)
                n.extend(xf.rot(v) for v in N)
            else:
                p.extend(P)
                n.extend(N)
            u.extend(U)
            i.extend(k + base for k in I)


def _uv_axes(a, grain):
    b, c = (a + 1) % 3, (a + 2) % 3
    if grain is not None and grain != a:
        return grain, (c if grain == b else b)
    return {1: (0, 2), 0: (2, 1), 2: (0, 1)}[a]


def box(m: Mesh, mat, mn, mx, xf: Xf | None = None, grain=None, off=None):
    """An axis-aligned box in local space, then `xf`. UVs are the local
    coordinates over the material's tile; `grain` names the axis a face's u
    follows (a board's length), where the face contains it."""
    tile = MATS[mat].get("tile", 1.0)
    if off is None:
        off = ((mn[0] * 0.37 + mn[2] * 0.61) % 1.0, (mn[1] * 0.53 + mn[2] * 0.29) % 1.0) if grain is not None else (0.0, 0.0)
    for a in range(3):
        b, c = (a + 1) % 3, (a + 2) % 3
        for s in (-1, 1):
            n = [0.0, 0.0, 0.0]
            n[a] = float(s)
            pts, uvs = [], []
            for (ib, ic) in ((0, 0), (1, 0), (1, 1), (0, 1)):
                p = [0.0, 0.0, 0.0]
                p[a] = mx[a] if s > 0 else mn[a]
                p[b] = mx[b] if ib else mn[b]
                p[c] = mx[c] if ic else mn[c]
                ua, va = _uv_axes(a, grain)
                uvs.append((p[ua] / tile + off[0], -p[va] / tile + off[1]))
                pts.append(tuple(p))
            nn = tuple(n)
            if xf:
                pts = [xf.pt(p) for p in pts]
                nn = xf.rot(nn)
            m.poly(mat, pts, [nn] * 4, uvs, nn)


def planks(m, mat, mn, mx, grain, across, n, seed, gap=0.005, xf=None):
    """Boards side by side: the box mn..mx cut into n along `across`, a gap
    between them, each its own slice of the texture and a hair off level."""
    rng = random.Random(seed)
    lo, hi = mn[across], mx[across]
    w = (hi - lo) / n
    for k in range(n):
        a = list(mn)
        b = list(mx)
        a[across] = lo + k * w + (gap / 2 if k else 0.0)
        b[across] = lo + (k + 1) * w - (gap / 2 if k < n - 1 else 0.0)
        b[1] += rng.uniform(-0.0015, 0.0015) if grain != 1 else 0.0
        box(m, mat, tuple(a), tuple(b), xf=xf, grain=grain, off=(rng.random(), rng.random()))


def obox(m, mat, center, size, xf: Xf, grain=None):
    """A box of `size` centred on the origin, placed by xf then `center`."""
    hx, hy, hz = size[0] / 2, size[1] / 2, size[2] / 2
    box(m, mat, (-hx, -hy, -hz), (hx, hy, hz), xf.then(move(center)), grain=grain)


def lathe(m: Mesh, mat, profile, segs=24, xf: Xf | None = None, smooth=True,
          facets=False, a0=0.0, a1=360.0, grain_up=True):
    """Revolve (radius, y) points, bottom to top, about +Y. `facets` keeps
    each segment flat (barrel staves)."""
    tile = MATS[mat].get("tile", 1.0)
    rmax = max(r for r, _ in profile) or 1.0
    # Profile normals in (r, y): per edge, and averaged per point if smooth.
    edge_n = []
    for (r0, y0), (r1, y1) in zip(profile, profile[1:]):
        dr, dy = r1 - r0, y1 - y0
        l = math.hypot(dr, dy) or 1.0
        edge_n.append((dy / l, -dr / l))
    xf = xf or Xf()
    for e, ((r0, y0), (r1, y1)) in enumerate(zip(profile, profile[1:])):
        if smooth:
            def pn(i):
                ns = [edge_n[k] for k in (i - 1, i) if 0 <= k < len(edge_n)]
                nr = sum(n[0] for n in ns); ny = sum(n[1] for n in ns)
                l = math.hypot(nr, ny) or 1.0
                return (nr / l, ny / l)
            n0, n1 = pn(e), pn(e + 1)
        else:
            n0 = n1 = edge_n[e]
        for i in range(segs):
            t0 = math.radians(a0 + (a1 - a0) * i / segs)
            t1 = math.radians(a0 + (a1 - a0) * (i + 1) / segs)
            tm = (t0 + t1) / 2
            pts, nrm, uvs = [], [], []
            for (r, y, nn, t) in ((r0, y0, n0, t0), (r0, y0, n0, t1), (r1, y1, n1, t1), (r1, y1, n1, t0)):
                pts.append((r * math.cos(t), y, r * math.sin(t)))
                tn = tm if facets else t
                nrm.append((nn[0] * math.cos(tn), nn[1], nn[0] * math.sin(tn)))
                arc = t * rmax
                uvs.append((y / tile, arc / tile) if grain_up else (arc / tile, y / tile))
            nr0 = n0 if not facets else edge_n[e]
            want = (nr0[0] * math.cos(tm), nr0[1], nr0[0] * math.sin(tm))
            pts = [xf.pt(p) for p in pts]
            nrm = [xf.rot(n) for n in nrm]
            m.poly(mat, pts, nrm, uvs, xf.rot(want))


def disc(m: Mesh, mat, r, y, up=True, segs=24, xf: Xf | None = None):
    tile = MATS[mat].get("tile", 1.0)
    xf = xf or Xf()
    n = (0.0, 1.0 if up else -1.0, 0.0)
    pts, uvs = [], []
    for i in range(segs):
        t = 2 * math.pi * i / segs
        p = (r * math.cos(t), y, r * math.sin(t))
        pts.append(xf.pt(p))
        uvs.append((p[0] / tile + 0.5, p[2] / tile + 0.5))
    nn = xf.rot(n)
    m.poly(mat, pts, [nn] * segs, uvs, nn)


def cylinder(m, mat, r, y0, y1, segs=20, xf=None, caps=(True, True), facets=False):
    lathe(m, mat, [(r, y0), (r, y1)], segs, xf, smooth=False, facets=facets)
    if caps[0]:
        disc(m, mat, r, y0, up=False, segs=segs, xf=xf)
    if caps[1]:
        disc(m, mat, r, y1, up=True, segs=segs, xf=xf)


def rod(m, mat, p0, p1, r, segs=10, caps=True):
    """A cylinder from p0 to p1."""
    d = sub(p1, p0)
    l = math.sqrt(dot(d, d))
    cylinder(m, mat, r, 0.0, l, segs, y_to(d, p0), caps=(caps, caps))


def torus(m: Mesh, mat, R, r, seg_major=32, seg_minor=8, xf: Xf | None = None, a0=0.0, a1=360.0):
    """A ring about +Y in the XZ plane (or an arc of one)."""
    tile = MATS[mat].get("tile", 1.0)
    xf = xf or Xf()
    for i in range(seg_major):
        u0 = math.radians(a0 + (a1 - a0) * i / seg_major)
        u1 = math.radians(a0 + (a1 - a0) * (i + 1) / seg_major)
        for j in range(seg_minor):
            v0 = 2 * math.pi * j / seg_minor
            v1 = 2 * math.pi * (j + 1) / seg_minor
            pts, nrm, uvs = [], [], []
            for (u, v) in ((u0, v0), (u1, v0), (u1, v1), (u0, v1)):
                c = (R * math.cos(u), 0.0, R * math.sin(u))
                n = (math.cos(v) * math.cos(u), math.sin(v), math.cos(v) * math.sin(u))
                pts.append(xf.pt(add(c, scl(n, r))))
                nrm.append(xf.rot(n))
                uvs.append((u * R / tile, v * r / tile))
            um, vm = (u0 + u1) / 2, (v0 + v1) / 2
            want = xf.rot((math.cos(vm) * math.cos(um), math.sin(vm), math.cos(vm) * math.sin(um)))
            m.poly(mat, pts, nrm, uvs, want)


def chain(m, p0, p1, link=0.034, R=0.014, r=0.0035):
    """Iron chain links from p0 to p1, each turned a quarter from the last."""
    d = sub(p1, p0)
    l = math.sqrt(dot(d, d))
    n = max(1, int(l / link))
    base = y_to(d)
    for k in range(n):
        c = add(p0, scl(d, (k + 0.5) / n))
        # a link: a ring stretched along the chain, as a torus in a plane
        # containing the chain's direction
        twist = rot_axis("y", 90 * (k % 2))
        stand = rot_axis("x", 90)  # ring's plane now holds Y
        xf = stand.then(twist).then(Xf(base.c, c))
        torus(m, "iron", R, r, 8, 4, xf)


# ─── The room ───────────────────────────────────────────────────────────────
#
# Main hall interior: x -6.85..6.85, z -4.85..9.85, floor at 0, ceiling at
# 4.2. Walls are 0.3 thick, centred on x = ±7, z = -5 and z = 10.

H = 4.2
WAINSCOT = 1.1
IN_W, IN_E, IN_N, IN_S = -6.85, 6.85, -4.85, 9.85

DOOR_N = (-0.75, 0.75, 0.0, 2.5)
DOOR_S = (-0.75, 0.75, 0.0, 2.5)
WIN_W = [(1.3, 2.5, 1.35, 2.75), (5.0, 6.2, 1.35, 2.75)]
WIN_S = [(-4.8, -3.6, 1.35, 2.55), (3.6, 4.8, 1.35, 2.55)]
BEAMS_Z = [-2.6, 0.9, 4.4, 7.9]
POSTS = [(2.0, 0.9), (2.0, 7.9)]
CHIMNEY_Z = (-4.4, -1.6)


class Wall:
    """One wall: runs along `along` ('x' or 'z'), occupies [t0, t1] across,
    with the room on the `inside` (+1: larger t) side."""

    def __init__(self, along, t0, t1, a0, a1, inside, holes=(), posts=(), blocked=()):
        self.along, self.t0, self.t1, self.a0, self.a1 = along, t0, t1, a0, a1
        self.inside, self.holes, self.posts, self.blocked = inside, list(holes), list(posts), list(blocked)

    def face(self):
        return self.t1 if self.inside > 0 else self.t0

    def at(self, a, y, t):
        return (a, y, t) if self.along == "x" else (t, y, a)

    def bx(self, m, mat, a0, a1, y0, y1, t0, t1, grain=None):
        p, q = self.at(a0, y0, t0), self.at(a1, y1, t1)
        mn = tuple(min(p[i], q[i]) for i in range(3))
        mx = tuple(max(p[i], q[i]) for i in range(3))
        g = None if grain is None else ({"a": 0 if self.along == "x" else 2, "y": 1}[grain])
        box(m, mat, mn, mx, grain=g)

    def inward(self, d):
        """The interior face pushed d into the room, as (t0, t1)."""
        f = self.face()
        return (f, f + d) if self.inside > 0 else (f - d, f)

    def hits(self, a0, a1, y0=0.0, y1=H):
        for (h0, h1, hy0, hy1) in self.holes:
            if a0 < h1 and a1 > h0 and y0 < hy1 and y1 > hy0:
                return True
        for (b0, b1) in self.blocked:
            if a0 < b1 and a1 > b0:
                return True
        return False

    def build(self, m: Mesh):
        aa = sorted({self.a0, self.a1, *[h[0] for h in self.holes], *[h[1] for h in self.holes]})
        yy = sorted({0.0, WAINSCOT, H + 0.15, *[h[2] for h in self.holes], *[h[3] for h in self.holes]})
        for a0, a1 in zip(aa, aa[1:]):
            for y0, y1 in zip(yy, yy[1:]):
                am, ym = (a0 + a1) / 2, (y0 + y1) / 2
                if any(h[0] < am < h[1] and h[2] < ym < h[3] for h in self.holes):
                    continue
                if y1 <= WAINSCOT + 1e-6:
                    t0, t1 = self.t0, self.t1
                    if self.inside > 0:
                        t1 += 0.05
                    else:
                        t0 -= 0.05
                    self.bx(m, "stone", a0, a1, y0, y1, t0, t1)
                else:
                    self.bx(m, "plaster", a0, a1, y0, y1, self.t0, self.t1)
        # Chair rail over the stone, top plate under the ceiling, posts between.
        rail = self.inward(0.085)
        plate = self.inward(0.075)
        post = self.inward(0.06)
        for (r0, r1) in self._runs(1.02, 1.18):
            self.bx(m, "timber", r0, r1, 1.04, 1.18, *rail, grain="a")
        for (r0, r1) in self._runs(H - 0.24, H):
            self.bx(m, "timber", r0, r1, H - 0.24, H, *plate, grain="a")
        for p in self.posts:
            if not self.hits(p - 0.11, p + 0.11, 1.18, H - 0.24):
                self.bx(m, "timber", p - 0.11, p + 0.11, 1.18, H - 0.24, *post, grain="y")

    def _runs(self, y0, y1):
        """Spans of the wall's length not cut by a hole between y0 and y1."""
        cuts = sorted([(h[0] - 0.13, h[1] + 0.13) for h in self.holes if y0 < h[3] and y1 > h[2]]
                      + [(b0, b1) for (b0, b1) in self.blocked])
        runs = []
        lo, hi = (IN_N, IN_S) if self.along == "z" else (IN_W, IN_E)
        cur = lo
        for c0, c1 in cuts:
            if c0 > cur:
                runs.append((cur, min(c0, hi)))
            cur = max(cur, c1)
        if cur < hi:
            runs.append((cur, hi))
        return [r for r in runs if r[1] - r[0] > 0.02]

    def window(self, m: Mesh, h, lattice=0.15):
        a0, a1, y0, y1 = h
        tm = (self.t0 + self.t1) / 2
        f, d = 0.075, 0.05            # frame width, half depth
        self.bx(m, "timber", a0, a0 + f, y0, y1, tm - d, tm + d, grain="y")
        self.bx(m, "timber", a1 - f, a1, y0, y1, tm - d, tm + d, grain="y")
        self.bx(m, "timber", a0, a1, y1 - f, y1, tm - d, tm + d, grain="a")
        self.bx(m, "timber", a0, a1, y0, y0 + f, tm - d, tm + d, grain="a")
        am = (a0 + a1) / 2
        ym = y0 + (y1 - y0) * 0.64
        self.bx(m, "timber", am - 0.03, am + 0.03, y0, y1, tm - 0.035, tm + 0.035, grain="y")
        self.bx(m, "timber", a0, a1, ym - 0.03, ym + 0.03, tm - 0.035, tm + 0.035, grain="a")
        # Sill: a deep board on the room side.
        st0, st1 = self.inward(0.14)
        if self.inside > 0:
            st0 = tm
        else:
            st1 = tm
        self.bx(m, "timber", a0 - 0.1, a1 + 0.1, y0 - 0.06, y0 + 0.005, st0, st1, grain="a")
        # Diamond leading: iron cames at ±45° in each pane.
        panes = [(a0 + f, am - 0.03, y0 + f, ym - 0.03), (am + 0.03, a1 - f, y0 + f, ym - 0.03),
                 (a0 + f, am - 0.03, ym + 0.03, y1 - f), (am + 0.03, a1 - f, ym + 0.03, y1 - f)]
        A = (1, 0, 0) if self.along == "x" else (0, 0, 1)
        Y = (0, 1, 0)
        Nn = cross(A, Y)
        for (pa0, pa1, py0, py1) in panes:
            for sgn in (1, -1):
                # lines a - sgn*y = k: walk k across the pane's range
                ks = [pa0 - sgn * py0, pa1 - sgn * py0, pa0 - sgn * py1, pa1 - sgn * py1]
                kmin, kmax = min(ks), max(ks)
                k = math.floor(kmin / lattice) * lattice
                while k <= kmax:
                    seg = _clip_line(k, sgn, pa0, pa1, py0, py1)
                    if seg:
                        (qa0, qy0), (qa1, qy1) = seg
                        L = math.hypot(qa1 - qa0, qy1 - qy0)
                        if L > 0.02:
                            ca, cy = (qa0 + qa1) / 2, (qy0 + qy1) / 2
                            dirv = norm(add(scl(A, qa1 - qa0), scl(Y, qy1 - qy0)))
                            perp = cross(Nn, dirv)
                            xf = Xf((dirv, perp, Nn), self.at(ca, cy, tm))
                            box(m, "iron", (-L / 2, -0.006, -0.006), (L / 2, 0.006, 0.006), xf)
                    k += lattice

    def door(self, m: Mesh, h):
        a0, a1, y0, y1 = h
        t0, t1 = self.t0 - 0.04, self.t1 + 0.04
        self.bx(m, "timber", a0 - 0.14, a0, 0.0, y1 + 0.2, t0, t1, grain="y")
        self.bx(m, "timber", a1, a1 + 0.14, 0.0, y1 + 0.2, t0, t1, grain="y")
        self.bx(m, "timber", a0 - 0.26, a1 + 0.26, y1, y1 + 0.24, t0, t1, grain="a")
        self.bx(m, "flag", a0 - 0.02, a1 + 0.02, -0.05, 0.025, self.t0 - 0.12, self.t1 + 0.12)


def _clip_line(k, sgn, a0, a1, y0, y1):
    """Clip the line a - sgn*y = k to the rectangle; two (a, y) points."""
    pts = []
    for y in (y0, y1):
        a = k + sgn * y
        if a0 - 1e-9 <= a <= a1 + 1e-9:
            pts.append((a, y))
    for a in (a0, a1):
        y = (a - k) * sgn
        if y0 - 1e-9 <= y <= y1 + 1e-9:
            pts.append((a, y))
    uniq = []
    for p in pts:
        if all(abs(p[0] - q[0]) + abs(p[1] - q[1]) > 1e-6 for q in uniq):
            uniq.append(p)
    if len(uniq) < 2:
        return None
    uniq.sort()
    return uniq[0], uniq[-1]


def build_shell() -> Mesh:
    m = Mesh()
    # Floor: oak boards running east-west.
    box(m, "oak", (IN_W, -0.05, IN_N), (IN_E, 0.012, IN_S), grain=0, off=(0.0, 0.0))

    walls = [
        Wall("z", -7.15, -6.85, -5.15, 10.15, +1, holes=WIN_W,
             posts=[-4.74, -1.25, 0.95, 2.85, 4.65, 6.55, 8.2, 9.74], blocked=[CHIMNEY_Z]),
        Wall("z", 6.85, 7.15, -5.15, 10.15, -1,
             posts=[-4.74, -3.2, 3.2, 5.6, 7.8, 9.74]),
        Wall("x", -5.15, -4.85, IN_W, IN_E, +1, holes=[DOOR_N], posts=[-6.74, -3.7, 3.7, 6.74]),
        Wall("x", 9.85, 10.15, IN_W, IN_E, -1, holes=[DOOR_S] + WIN_S, posts=[-6.74, -2.4, 2.4, 6.74]),
    ]
    for w in walls:
        w.build(m)
    for h in WIN_W:
        walls[0].window(m, h)
    for h in WIN_S:
        walls[3].window(m, h)
    walls[2].door(m, DOOR_N)
    walls[3].door(m, DOOR_S)

    # Ceiling, beams across the room, joists along it.
    box(m, "plaster_smoked", (IN_W, H, IN_N), (IN_E, H + 0.15, IN_S))
    for z in BEAMS_Z:
        box(m, "timber", (IN_W, H - 0.38, z - 0.16), (IN_E, H, z + 0.16), grain=0)
    x = IN_W + 0.45
    while x < IN_E - 0.3:
        box(m, "timber", (x - 0.06, H - 0.12, IN_N), (x + 0.06, H, IN_S), grain=2)
        x += 0.8
    # Two posts under the beams, braced to them.
    for (px, pz) in POSTS:
        box(m, "timber", (px - 0.15, 0.0, pz - 0.15), (px + 0.15, H - 0.38, pz + 0.15), grain=1)
        box(m, "flag", (px - 0.24, 0.0, pz - 0.24), (px + 0.24, 0.06, pz + 0.24))
        for s in (1, -1):
            obox(m, "timber", (px + s * 0.39, H - 0.66, pz), (0.78, 0.12, 0.12),
                 rot_axis("z", 45 * s), grain=0)

    # The alcove behind the kitchen door, so the door opens onto a room.
    box(m, "oak", (-1.3, -0.05, -6.4), (1.3, 0.012, -5.15), grain=0)
    box(m, "stone", (-1.45, 0.0, -6.55), (1.45, 1.1, -6.4))
    box(m, "plaster", (-1.45, 1.1, -6.55), (1.45, 2.9, -6.4))
    for x0 in (-1.45, 1.3):
        box(m, "plaster", (x0, 0.0, -6.4), (x0 + 0.15, 2.9, -5.15))
    box(m, "plaster", (-1.45, 2.75, -6.55), (1.45, 2.9, -5.15))

    # Outside: a gable roof of dark shingles, its ends plastered, and the
    # chimney stack rising out of the west wall.
    eave_x, eave_y, ridge_y = 7.6, H + 0.15, 7.3
    run, rise = eave_x, ridge_y - eave_y
    ang = math.degrees(math.atan2(rise, run))
    L = math.hypot(run, rise) + 0.25
    for s in (-1, 1):
        cx, cy = s * run / 2, eave_y + rise / 2 + 0.08
        obox(m, "shingle", (cx, cy, 2.5), (L, 0.14, 16.0), rot_axis("z", -s * ang), grain=0)
    for z, nz in ((-5.15, -1.0), (10.15, 1.0)):
        tri = [(-7.15, eave_y, z), (7.15, eave_y, z), (0.0, ridge_y - 0.1, z)]
        uv = [(p[0] / 2.6, -p[1] / 2.6) for p in tri]
        m.poly("plaster", tri, [(0, 0, nz)] * 3, uv, (0, 0, nz))
    box(m, "stone", (-7.95, 0.0, -4.0), (-7.15, 8.2, -2.0))
    return m


def build_hearth() -> Mesh:
    """The great hearth, in its entity's frame: (-6.5, 0, -3) in the hall.
    The wall's face is at x = -0.35 here; the firebox opens toward +x."""
    m = Mesh()
    back, front = -0.35, 0.55
    zj, zo = 0.78, 1.4          # firebox half-width, breast half-width
    top = 1.25
    box(m, "stone", (back, 0.0, -zo), (front, top, -zj))
    box(m, "stone", (back, 0.0, zj), (front, top, zo))
    box(m, "stone", (back, top, -zo), (front, H, zo))
    # Firebox lining, black with soot, and its floor.
    box(m, "soot", (back, 0.0, -zj), (-0.18, top, zj))
    box(m, "soot", (-0.18, 0.0, -zj), (front - 0.02, top, -zj + 0.03))
    box(m, "soot", (-0.18, 0.0, zj - 0.03), (front - 0.02, top, zj))
    box(m, "soot", (-0.18, top - 0.04, -zj), (front - 0.02, top, zj))
    box(m, "soot", (-0.18, 0.0, -zj), (front, 0.03, zj))
    # Oak bressumer over the opening, the mantel shelf on it.
    box(m, "timber", (front - 0.08, 1.2, -zo - 0.06), (front + 0.1, 1.46, zo + 0.06), grain=2)
    box(m, "timber", (front - 0.12, 1.46, -zo - 0.16), (front + 0.24, 1.53, zo + 0.16), grain=2)
    # Hearthstone before it.
    box(m, "flag", (front, -0.02, -zo + 0.05), (front + 0.72, 0.05, zo - 0.05))
    # Firedogs.
    for z in (-0.36, 0.36):
        box(m, "iron", (-0.1, 0.03, z - 0.02), (0.42, 0.07, z + 0.02))
        rod(m, "iron", (0.4, 0.03, z), (0.4, 0.34, z), 0.022)
        lathe(m, "iron", [(0.0, 0.34), (0.035, 0.36), (0.03, 0.4), (0.0, 0.42)], 12,
              move((0.4, 0.0, z)))
    # Logs across the firedogs, one leant on them, ember-bright at the ends.
    logs = [((0.06, 0.13, -0.5), (0.06, 0.13, 0.5), 0.085),
            ((0.26, 0.12, -0.46), (0.22, 0.12, 0.48), 0.075),
            ((-0.08, 0.24, -0.42), (0.16, 0.2, 0.46), 0.07)]
    for p0, p1, r in logs:
        rod(m, "bark", p0, p1, r, 12, caps=False)
        d = norm(sub(p1, p0))
        for p, sgn in ((p0, -1), (p1, 1)):
            disc(m, "ember", r * 0.92, 0.0, up=True, segs=12, xf=y_to(scl(d, sgn), add(p, scl(d, sgn * 0.002))))
    # Ember bed and a scatter of coals.
    box(m, "ember", (-0.12, 0.03, -0.5), (0.4, 0.06, 0.5))
    rng = random.Random(1795)
    for _ in range(26):
        cx, cz = rng.uniform(-0.1, 0.42), rng.uniform(-0.5, 0.5)
        s = rng.uniform(0.025, 0.05)
        obox(m, "ember" if rng.random() < 0.6 else "soot", (cx, 0.07, cz), (s, s * 0.7, s),
             rot_axis("y", rng.uniform(0, 90)))
    # On the mantel: candlesticks, pewter plates, a stoneware jug.
    for z in (-1.25, 1.25):
        m.merge(build_candle(stick=True), move((front + 0.05, 1.53, z)))
    for z, tilt in ((-0.45, 12), (0.5, -10)):
        xf = rot_axis("x", 90).then(rot_axis("y", 90)).then(rot_axis("z", tilt)).then(move((front - 0.06, 1.53 + 0.14, z)))
        cylinder(m, "pewter", 0.13, -0.006, 0.006, 28, xf)
        torus(m, "pewter", 0.125, 0.008, 28, 6, rot_axis("x", 0).then(xf))
    m.merge(build_jug(), move((front + 0.06, 1.53, 0.05)))
    return m


def build_passage_stone() -> Mesh:
    """The showcase's 'strange wall': a block of colder, older stone."""
    m = Mesh()
    box(m, "coldstone", (-0.5, -1.2, -0.5), (0.5, 1.2, 0.5))
    box(m, "soot", (0.5, -0.02, -0.42), (0.505, 0.02, 0.42))
    box(m, "soot", (0.5, -1.0, -0.02), (0.505, 1.0, 0.02))
    return m


# ─── Furniture ──────────────────────────────────────────────────────────────

def build_table() -> Mesh:
    """A square plank table, 1.2 m, top at 0.75."""
    m = Mesh()
    planks(m, "board", (-0.6, 0.69, -0.6), (0.6, 0.75, 0.6), 0, 2, 5, seed=11)
    for s in (-1, 1):
        box(m, "timber", (-0.48, 0.55, s * 0.47 - 0.025), (0.48, 0.69, s * 0.47 + 0.025), grain=0)
        box(m, "timber", (s * 0.47 - 0.025, 0.55, -0.48), (s * 0.47 + 0.025, 0.69, 0.48), grain=2)
    for sx in (-1, 1):
        for sz in (-1, 1):
            box(m, "timber", (sx * 0.5 - 0.045, 0.0, sz * 0.5 - 0.045), (sx * 0.5 + 0.045, 0.69, sz * 0.5 + 0.045), grain=1)
        box(m, "timber", (sx * 0.5 - 0.025, 0.1, -0.46), (sx * 0.5 + 0.025, 0.16, 0.46), grain=2)
    box(m, "timber", (-0.46, 0.1, -0.025), (0.46, 0.16, 0.025), grain=0)
    return m


def build_table_long() -> Mesh:
    """A trestle table, 1.6 x 0.8, top at 0.75."""
    m = Mesh()
    planks(m, "board", (-0.8, 0.69, -0.4), (0.8, 0.75, 0.4), 0, 2, 3, seed=12)
    for s in (-1, 1):
        x = s * 0.6
        box(m, "timber", (x - 0.05, 0.0, -0.36), (x + 0.05, 0.08, 0.36), grain=2)
        box(m, "timber", (x - 0.04, 0.08, -0.22), (x + 0.04, 0.64, 0.22), grain=1)
        box(m, "timber", (x - 0.05, 0.63, -0.36), (x + 0.05, 0.69, 0.36), grain=2)
        box(m, "timber", (x + s * 0.04, 0.3, -0.03), (x + s * 0.1, 0.44, 0.03))
    box(m, "timber", (-0.7, 0.32, -0.035), (0.7, 0.42, 0.035), grain=0)
    return m


def build_bench() -> Mesh:
    """A slab-legged bench, 1.5 m along x, seat at 0.46."""
    m = Mesh()
    planks(m, "board", (-0.75, 0.4, -0.16), (0.75, 0.46, 0.16), 0, 2, 2, seed=13)
    for s in (-1, 1):
        x = s * 0.56
        box(m, "timber", (x - 0.035, 0.0, -0.14), (x + 0.035, 0.4, 0.14), grain=1)
        box(m, "timber", (x - 0.06, 0.0, -0.15), (x + 0.06, 0.05, 0.15), grain=2)
    box(m, "timber", (-0.62, 0.14, -0.025), (0.62, 0.21, 0.025), grain=0)
    return m


def build_stool(seat=0.48) -> Mesh:
    """A three-legged stool, its seat at `seat` (0.76 at the bar)."""
    m = Mesh()
    cylinder(m, "board", 0.17, seat - 0.05, seat, 24)
    splay = 0.19 + 0.08 * (seat - 0.48) / 0.28
    legs = []
    for k in range(3):
        a = math.radians(90 + 120 * k)
        top = (0.1 * math.cos(a), seat - 0.04, 0.1 * math.sin(a))
        foot = (splay * math.cos(a), 0.0, splay * math.sin(a))
        rod(m, "timber", foot, top, 0.022, 10)
        legs.append((foot, top))
    for f in ((0.35,) if seat < 0.6 else (0.3, 0.62)):
        for k in range(3):
            (f0, t0), (f1, t1) = legs[k], legs[(k + 1) % 3]
            p0 = add(f0, scl(sub(t0, f0), f))
            p1 = add(f1, scl(sub(t1, f1), f))
            rod(m, "timber", p0, p1, 0.013, 8)
    return m


def barrel_profile(h, r_end, r_mid, rings=10):
    return [(r_end + (r_mid - r_end) * math.sin(math.pi * k / rings), h * k / rings) for k in range(rings + 1)]


def add_barrel(m, h=0.9, r_end=0.25, r_mid=0.3, xf=None, hoops=(0.08, 0.2, 0.7, 0.82)):
    xf = xf or Xf()
    prof = barrel_profile(h, r_end, r_mid)
    lathe(m, "stave", prof, 16, xf, smooth=True, facets=True)
    def r_at(y):
        return r_end + (r_mid - r_end) * math.sin(math.pi * y / h)
    for f in hoops:
        y = f * h
        r = r_at(y) + 0.006
        lathe(m, "iron", [(r, y - 0.018), (r + 0.004, y), (r, y + 0.018)], 32, xf, smooth=True)
    inset = 0.025
    for y, up in ((h - inset, True), (inset, False)):
        disc(m, "stave", r_end - 0.008, y, up=up, segs=24, xf=xf)
    # the chime: the staves' ends standing proud of the heads
    lathe(m, "stave", [(r_end - 0.012, h - inset), (r_end - 0.012, h), (r_end, h)], 24, xf, smooth=False)
    lathe(m, "stave", [(r_end, 0.0), (r_end - 0.012, 0.0), (r_end - 0.012, inset)], 24, xf, smooth=False)


def build_barrel() -> Mesh:
    m = Mesh()
    add_barrel(m)
    return m


def build_bar() -> Mesh:
    """The bar counter, its entity at (5, 0, 0): 1 m deep, 6 m long, top at 1.1.
    The room is on its -x side; the bartender stands on +x."""
    m = Mesh()
    box(m, "timber", (-0.42, 0.0, -2.95), (0.45, 0.12, 2.95), grain=2)
    planks(m, "board_dark", (-0.44, 0.12, -2.95), (0.45, 1.03, 2.95), 1, 2, 24, seed=14, gap=0.003)
    # Framed panels on the room side.
    n = 8
    zs = [-2.95 + 5.9 * k / n for k in range(n + 1)]
    for z in zs:
        box(m, "timber", (-0.5, 0.12, z - 0.05), (-0.44, 1.03, z + 0.05), grain=1)
    for (y0, y1) in ((0.12, 0.22), (0.93, 1.03)):
        box(m, "timber", (-0.5, y0, -2.95), (-0.44, y1, 2.95), grain=2)
    for z0, z1 in zip(zs, zs[1:]):
        box(m, "timber", (-0.475, 0.22, z0 + 0.05), (-0.44, 0.93, z1 - 0.05), grain=1)
    # The top, overhanging the room side.
    planks(m, "board", (-0.62, 1.03, -3.08), (0.55, 1.1, 3.08), 2, 0, 3, seed=15)
    # Brass foot rail on its brackets.
    rod(m, "brass", (-0.64, 0.2, -2.9), (-0.64, 0.2, 2.9), 0.024, 16)
    for z in (-2.7, -1.35, 0.0, 1.35, 2.7):
        rod(m, "brass", (-0.64, 0.2, z), (-0.5, 0.26, z), 0.012, 8)
    # A keg on its cradle at the north end, its tap toward the room.
    kx, ky, kz = -0.05, 1.1 + 0.03, -2.45
    keg = rot_axis("z", 90).then(move((kx + 0.22, ky + 0.17, kz)))
    add_barrel(m, h=0.44, r_end=0.13, r_mid=0.16, xf=keg, hoops=(0.12, 0.88))
    for dz in (-0.12, 0.12):
        box(m, "timber", (kx - 0.24, ky - 0.03, kz + dz - 0.03), (kx + 0.24, ky + 0.06, kz + dz + 0.03), grain=0)
    rod(m, "brass", (kx - 0.22, ky + 0.13, kz), (kx - 0.32, ky + 0.13, kz), 0.014, 10)
    rod(m, "brass", (kx - 0.31, ky + 0.13, kz), (kx - 0.31, ky + 0.05, kz), 0.01, 8)
    rod(m, "brass", (kx - 0.31, ky + 0.13, kz), (kx - 0.31, ky + 0.22, kz), 0.006, 6)
    return m


def build_bottle(rng, h=None):
    m = Mesh()
    kind = rng.random()
    if kind < 0.55:
        h = h or rng.uniform(0.26, 0.32)
        r = rng.uniform(0.033, 0.042)
        prof = [(0.0, 0.0), (r, 0.0), (r, h * 0.62), (r * 0.8, h * 0.72), (0.013, h * 0.82), (0.012, h * 0.96), (0.014, h * 0.97), (0.014, h)]
        glass = rng.choice(["glass_green", "glass_green", "glass_dark", "glass_amber"])
    elif kind < 0.8:
        h = h or rng.uniform(0.16, 0.2)
        r = rng.uniform(0.05, 0.06)
        prof = [(0.0, 0.0), (r * 0.8, 0.0), (r, h * 0.2), (r, h * 0.45), (r * 0.6, h * 0.7), (0.016, h * 0.85), (0.016, h)]
        glass = rng.choice(["glass_amber", "glass_clear", "glass_dark"])
    else:
        return build_jug(rng)
    lathe(m, glass, prof, 10, smooth=True)
    cylinder(m, "cork", prof[-1][0] * 0.9, h, h + 0.022, 10)
    return m


def build_jug(rng=None):
    rng = rng or random.Random(7)
    m = Mesh()
    s = rng.uniform(0.9, 1.15)
    prof = [(0.0, 0.0), (0.055 * s, 0.0), (0.075 * s, 0.05 * s), (0.072 * s, 0.12 * s), (0.04 * s, 0.17 * s),
            (0.03 * s, 0.19 * s), (0.034 * s, 0.205 * s), (0.0, 0.205 * s)]
    mat = rng.choice(["crock", "crock", "crock_dark"])
    lathe(m, mat, prof, 12, smooth=True)
    xf = rot_axis("x", 90).then(move((0.07 * s, 0.13 * s, 0.0)))
    torus(m, mat, 0.04 * s, 0.009, 10, 6, xf, a0=-100, a1=100)
    return m


def build_backbar() -> Mesh:
    """Shelves of bottles against the east wall; entity at (6.6, 0, 0), wall at x = +0.25."""
    m = Mesh()
    x0, x1 = -0.2, 0.25
    planks(m, "board_dark", (x0, 0.0, -2.9), (x1, 0.92, 2.9), 1, 2, 22, seed=16, gap=0.003)
    for z in (-2.9, -1.45, 0.0, 1.45, 2.9):
        box(m, "timber", (x0 - 0.02, 0.0, z - 0.04), (x0, 0.92, z + 0.04), grain=1)
    box(m, "timber", (x0 - 0.04, 0.92, -2.95), (x1, 0.98, 2.95), grain=2)
    shelves = [1.48, 1.98, 2.48]
    for z in (-2.9, -0.98, 0.98, 2.9):
        box(m, "timber", (x0 + 0.04, 0.98, z - 0.03), (x1, 2.84, z + 0.03), grain=1)
    for y in shelves:
        box(m, "timber", (x0 + 0.04, y - 0.04, -2.9), (x1, y, 2.9), grain=2)
    box(m, "timber", (x0 + 0.02, 2.84, -2.95), (x1, 2.94, 2.95), grain=2)
    rng = random.Random(295)
    for y in [0.98] + shelves:
        z = -2.82
        while z < 2.8:
            b = build_bottle(rng)
            xb = rng.uniform(-0.02, 0.12)
            m.merge(b, rot_axis("y", rng.uniform(0, 360)).then(move((xb, y, z))))
            z += rng.uniform(0.11, 0.2)
            if rng.random() < 0.12:
                z += 0.25
    return m


def build_tankard(wood=False) -> Mesh:
    m = Mesh()
    if wood:
        lathe(m, "stave", [(0.0, 0.0), (0.045, 0.0), (0.045, 0.002), (0.04, 0.13)], 12, smooth=True, facets=True)
        for y in (0.02, 0.11):
            lathe(m, "iron", [(0.0445 - 0.4 * (y / 0.13) * 0.012, y - 0.008), (0.047 - (y / 0.13) * 0.005, y), (0.0445 - (y / 0.13) * 0.005, y + 0.008)], 20)
        lathe(m, "stave", [(0.04, 0.13), (0.034, 0.13), (0.034, 0.03)], 12, smooth=False)
        rim = 0.034
    else:
        lathe(m, "pewter", [(0.0, 0.0), (0.046, 0.0), (0.047, 0.006), (0.041, 0.02), (0.039, 0.11), (0.043, 0.122)], 24)
        lathe(m, "pewter", [(0.043, 0.122), (0.037, 0.122), (0.035, 0.03)], 24, smooth=False)
        rim = 0.035
    disc(m, "ale", rim, 0.112, up=True, segs=16)
    hx = rot_axis("x", 90).then(move((0.043, 0.07, 0.0)))
    torus(m, "iron" if wood else "pewter", 0.036, 0.007, 12, 6, hx, a0=-80, a1=80)
    return m


def build_candle(stick=False) -> Mesh:
    """A pillar candle in a pewter dish (or on a pricket stick), flame lit."""
    m = Mesh()
    if stick:
        lathe(m, "pewter", [(0.0, 0.0), (0.055, 0.0), (0.05, 0.015), (0.015, 0.03), (0.012, 0.16),
                            (0.03, 0.17), (0.035, 0.18), (0.0, 0.18)], 20)
        base = 0.18
        r, h = 0.016, 0.16
    else:
        lathe(m, "pewter", [(0.0, 0.0), (0.06, 0.0), (0.065, 0.012), (0.058, 0.014), (0.0, 0.01)], 20)
        base = 0.01
        r, h = 0.026, 0.11
    lathe(m, "wax", [(0.0, base), (r, base), (r, base + h * 0.92), (r * 0.8, base + h), (0.0, base + h - 0.004)], 16)
    for k in range(3):
        a = math.radians(40 + 130 * k)
        lathe(m, "wax", [(0.0, base + h * 0.35), (0.006, base + h * 0.55), (0.004, base + h * 0.97), (0.0, base + h)], 6,
              move((r * math.cos(a), 0.0, r * math.sin(a))))
    rod(m, "wick", (0, base + h - 0.004, 0), (0.002, base + h + 0.012, 0), 0.0015, 4)
    add_flame(m, (0.002, base + h + 0.008, 0.0))
    return m


def add_flame(m, at, size=1.0):
    s = size
    prof = [(0.0, 0.0), (0.006 * s, 0.006 * s), (0.008 * s, 0.014 * s), (0.006 * s, 0.026 * s), (0.0, 0.042 * s)]
    lathe(m, "flame", prof, 10, move(at))


def build_chandelier() -> Mesh:
    """A wrought iron ring of eight candles. Origin at the hook it hangs from;
    the ring is 1.15 m below it."""
    m = Mesh()
    ring_y, R = -1.15, 0.62
    lathe(m, "iron", [(0.0, -0.02), (0.03, -0.03), (0.02, -0.06), (0.0, -0.06)], 10)
    torus(m, "iron", 0.03, 0.006, 10, 6, rot_axis("x", 90).then(move((0, -0.09, 0))))
    hub = (0.0, ring_y + 0.42, 0.0)
    for k in range(4):
        a = math.radians(45 + 90 * k)
        on_ring = (R * math.cos(a), ring_y + 0.02, R * math.sin(a))
        chain(m, hub, on_ring)
    chain(m, (0.0, -0.11, 0.0), hub)
    torus(m, "iron", 0.04, 0.008, 12, 6, move(hub))
    torus(m, "iron", R, 0.022, 48, 8, move((0, ring_y, 0)))
    torus(m, "iron", R * 0.45, 0.014, 32, 6, move((0, ring_y - 0.03, 0)))
    for k in range(4):
        a = math.radians(90 * k)
        rod(m, "iron", (R * 0.45 * math.cos(a), ring_y - 0.03, R * 0.45 * math.sin(a)),
            (R * math.cos(a), ring_y, R * math.sin(a)), 0.01, 6)
    lathe(m, "iron", [(0.0, ring_y - 0.2), (0.025, ring_y - 0.14), (0.012, ring_y - 0.06), (0.0, ring_y - 0.04)], 10)
    for k in range(4):
        a = math.radians(90 * k)
        rod(m, "iron", (0, ring_y - 0.08, 0), (R * 0.45 * math.cos(a), ring_y - 0.03, R * 0.45 * math.sin(a)), 0.008, 6)
    for k in range(8):
        a = math.radians(22.5 + 45 * k)
        p = (R * math.cos(a), ring_y + 0.01, R * math.sin(a))
        lathe(m, "iron", [(0.0, -0.005), (0.05, 0.0), (0.05, 0.012), (0.044, 0.012), (0.0, 0.004)], 14, move(p))
        h = 0.08 + 0.05 * ((k * 37) % 5) / 4
        lathe(m, "wax", [(0.0, 0.0), (0.018, 0.0), (0.018, h), (0.014, h + 0.006), (0.0, h + 0.004)], 10, move(p))
        rod(m, "wick", add(p, (0, h + 0.002, 0)), add(p, (0, h + 0.014, 0)), 0.0015, 4)
        add_flame(m, add(p, (0, h + 0.01, 0)), 1.1)
    return m


def build_sconce() -> Mesh:
    """An iron wall sconce with a candle. Origin on the wall; it reaches +x."""
    m = Mesh()
    box(m, "iron", (0.0, -0.16, -0.06), (0.012, 0.16, 0.06))
    box(m, "iron", (0.0, -0.17, -0.025), (0.016, -0.13, 0.025))
    rod(m, "iron", (0.01, -0.1, 0.0), (0.16, -0.06, 0.0), 0.009, 8)
    rod(m, "iron", (0.01, 0.06, 0.0), (0.16, -0.05, 0.0), 0.007, 8)
    lathe(m, "iron", [(0.0, -0.075), (0.06, -0.065), (0.06, -0.055), (0.052, -0.055), (0.0, -0.062)], 16, move((0.17, 0.0, 0.0)))
    c = build_candle()
    for mat, (P, N, U, I) in c.parts.items():
        if mat == "pewter":
            continue
        sub_m = Mesh()
        sub_m.parts[mat] = (P, N, U, I)
        m.merge(sub_m, move((0.17, -0.07, 0.0)))
    return m


def build_lantern() -> Mesh:
    """A hanging lantern. Origin at the hook; 0.85 m of chain, then the lantern."""
    m = Mesh()
    top = -0.85
    chain(m, (0.0, -0.02, 0.0), (0.0, top + 0.02, 0.0))
    torus(m, "iron", 0.035, 0.007, 12, 6, rot_axis("x", 90).then(move((0, top - 0.01, 0))))
    w, hgt = 0.11, 0.3
    y1, y0 = top - 0.1, top - 0.1 - hgt
    lathe(m, "iron", [(0.0, y1 + 0.09), (0.03, y1 + 0.08), (0.05, y1 + 0.05), (w * 1.5, y1), (w * 1.35, y1 - 0.01), (0.0, y1 - 0.01)], 4, rot_axis("y", 45), smooth=False)
    box(m, "iron", (-w * 1.05, y0 - 0.03, -w * 1.05), (w * 1.05, y0, w * 1.05))
    for sx in (-1, 1):
        for sz in (-1, 1):
            box(m, "iron", (sx * w - 0.008, y0, sz * w - 0.008), (sx * w + 0.008, y1, sz * w + 0.008))
    for (a, n) in (("x", 1), ("x", -1), ("z", 1), ("z", -1)):
        if a == "x":
            box(m, "lantern_glass", (n * w - 0.002, y0, -w), (n * w + 0.002, y1, w))
        else:
            box(m, "lantern_glass", (-w, y0, n * w - 0.002), (w, y1, n * w + 0.002))
    c = build_candle()
    m.merge(c, move((0.0, y0, 0.0)))
    return m


def build_door() -> Mesh:
    """A ledged and braced plank door, 1.5 x 2.5. Origin at its hinge."""
    m = Mesh()
    planks(m, "board", (0.01, 0.0, -0.035), (1.49, 2.48, 0.035), 1, 0, 6, seed=17, gap=0.006)
    # a tongue inside every joint, so the gaps between boards show shadow, not sky
    box(m, "board_dark", (0.02, 0.01, -0.02), (1.48, 2.47, 0.02), grain=1)
    for side in (1, -1):
        zs = (0.035, 0.065) if side > 0 else (-0.065, -0.035)
        ys = (0.32, 1.24, 2.16)
        for y in ys:
            box(m, "timber", (0.07, y - 0.08, zs[0]), (1.43, y + 0.08, zs[1]), grain=0)
        for ya, yb in zip(ys, ys[1:]):
            p0, p1 = (0.16, ya + 0.08), (1.34, yb - 0.08)
            L = math.hypot(p1[0] - p0[0], p1[1] - p0[1])
            ang = math.degrees(math.atan2(p1[1] - p0[1], p1[0] - p0[0]))
            obox(m, "timber", ((p0[0] + p1[0]) / 2, (p0[1] + p1[1]) / 2, (zs[0] + zs[1]) / 2), (L, 0.13, 0.03),
                 rot_axis("z", ang), grain=0)
        # strap hinges and studs on the outer face
    for y in (0.32, 2.16):
        box(m, "iron", (-0.02, y - 0.03, -0.075), (1.05, y + 0.03, -0.065))
        box(m, "iron", (-0.02, y - 0.03, 0.065), (0.25, y + 0.03, 0.075))
        for x in (0.15, 0.45, 0.75, 1.0):
            lathe(m, "iron", [(0.0, 0.0), (0.012, 0.0), (0.0, 0.008)], 8, rot_axis("x", -90).then(move((x, y, -0.075))))
    for z, s in ((0.075, 1), (-0.075, -1)):
        lathe(m, "iron", [(0.0, 0.0), (0.03, 0.0), (0.03, 0.006), (0.0, 0.01)], 12, rot_axis("x", 90 * s).then(move((1.3, 1.05, z))))
        torus(m, "iron", 0.045, 0.007, 16, 6, rot_axis("z", 90).then(move((1.3, 1.0, z + s * 0.012))))
    return m


# ─── glTF ───────────────────────────────────────────────────────────────────

def write_glb(mesh: Mesh, path: Path, name: str):
    gltf = {"asset": {"version": "2.0", "generator": "flint demo/tools/tavern_build.py"},
            "scene": 0, "scenes": [{"nodes": [0]}],
            "nodes": [{"name": name, "mesh": 0}],
            "meshes": [{"name": name, "primitives": []}],
            "materials": [], "accessors": [], "bufferViews": [], "buffers": []}
    blob = bytearray()
    images, textures = {}, {}

    def view(data: bytes, target):
        while len(blob) % 4:
            blob.append(0)
        off = len(blob)
        blob.extend(data)
        gltf["bufferViews"].append({"buffer": 0, "byteOffset": off, "byteLength": len(data), "target": target})
        return len(gltf["bufferViews"]) - 1

    def accessor(v, ctype, count, typ, mn=None, mx=None):
        a = {"bufferView": v, "componentType": ctype, "count": count, "type": typ}
        if mn is not None:
            a["min"], a["max"] = mn, mx
        gltf["accessors"].append(a)
        return len(gltf["accessors"]) - 1

    def texture(file):
        if file not in textures:
            gltf.setdefault("images", []).append({"uri": TEX_URI + file, "mimeType": "image/jpeg"})
            gltf.setdefault("samplers", [{"magFilter": 9729, "minFilter": 9987, "wrapS": 10497, "wrapT": 10497}])
            gltf.setdefault("textures", []).append({"source": len(gltf["images"]) - 1, "sampler": 0})
            textures[file] = len(gltf["textures"]) - 1
        return {"index": textures[file]}

    uses_strength = False
    for mat in sorted(mesh.parts):
        P, N, U, I = mesh.parts[mat]
        if not I:
            continue
        # Weld vertices that agree on position, normal and uv (to ~0.1 mm):
        # neighbouring quads of a lathe or torus share their edges.
        weld, remap, P2, N2, U2 = {}, [], [], [], []
        for p, n, u in zip(P, N, U):
            n = norm(n)
            key = (tuple(round(c, 4) for c in p), tuple(round(c, 3) for c in n), tuple(round(c, 4) for c in u))
            k = weld.get(key)
            if k is None:
                k = weld[key] = len(P2)
                P2.append(p); N2.append(n); U2.append(u)
            remap.append(k)
        P, N, U, I = P2, N2, U2, [remap[i] for i in I]
        spec = MATS[mat]
        pbr = {}
        tint = spec.get("tint", (1.0, 1.0, 1.0))
        if "tex" in spec:
            t = spec["tex"]
            pbr["baseColorTexture"] = texture(f"tavern_{t}_color.jpg")
            pbr["metallicRoughnessTexture"] = texture(f"tavern_{t}_mr.jpg")
            pbr["baseColorFactor"] = [*tint, 1.0]
            pbr["metallicFactor"] = spec.get("metal", 0.0)
            pbr["roughnessFactor"] = spec.get("rough", 1.0)
        else:
            pbr["baseColorFactor"] = [*spec["color"], 1.0]
            pbr["metallicFactor"] = spec.get("metal", 0.0)
            pbr["roughnessFactor"] = spec.get("rough", 0.7)
        md = {"name": mat, "pbrMetallicRoughness": pbr}
        if "tex" in spec:
            md["normalTexture"] = texture(f"tavern_{spec['tex']}_normal.jpg")
        if "emit" in spec:
            md["emissiveFactor"] = list(spec["emit"])
            if spec.get("strength", 1.0) != 1.0:
                md["extensions"] = {"KHR_materials_emissive_strength": {"emissiveStrength": spec["strength"]}}
                uses_strength = True
        gltf["materials"].append(md)
        mi = len(gltf["materials"]) - 1

        pos = struct.pack(f"<{len(P) * 3}f", *[c for p in P for c in p])
        nrm = struct.pack(f"<{len(N) * 3}f", *[c for n in N for c in norm(n)])
        uv = struct.pack(f"<{len(U) * 2}f", *[c for u in U for c in u])
        idx = struct.pack(f"<{len(I)}I", *I)
        mn = [min(p[i] for p in P) for i in range(3)]
        mx = [max(p[i] for p in P) for i in range(3)]
        a_pos = accessor(view(pos, 34962), 5126, len(P), "VEC3", mn, mx)
        a_nrm = accessor(view(nrm, 34962), 5126, len(N), "VEC3")
        a_uv = accessor(view(uv, 34962), 5126, len(U), "VEC2")
        a_idx = accessor(view(idx, 34963), 5125, len(I), "SCALAR")
        gltf["meshes"][0]["primitives"].append(
            {"attributes": {"POSITION": a_pos, "NORMAL": a_nrm, "TEXCOORD_0": a_uv},
             "indices": a_idx, "material": mi})
    if uses_strength:
        gltf["extensionsUsed"] = ["KHR_materials_emissive_strength"]
    while len(blob) % 4:
        blob.append(0)
    gltf["buffers"].append({"byteLength": len(blob)})
    js = json.dumps(gltf, separators=(",", ":")).encode()
    while len(js) % 4:
        js += b" "
    out = bytearray()
    out += struct.pack("<III", 0x46546C67, 2, 12 + 8 + len(js) + 8 + len(blob))
    out += struct.pack("<II", len(js), 0x4E4F534A) + js
    out += struct.pack("<II", len(blob), 0x004E4942) + blob
    path.write_bytes(bytes(out))
    tris = sum(len(p[3]) // 3 for p in mesh.parts.values())
    print(f"  {path.relative_to(REPO).as_posix():44s} {tris:7d} tris  {len(out) / 1024:7.1f} KiB")


MODELS = {
    "tavern_shell": build_shell,
    "tavern_hearth": build_hearth,
    "tavern_passage_stone": build_passage_stone,
    "tavern_table": build_table,
    "tavern_table_long": build_table_long,
    "tavern_bench": build_bench,
    "tavern_stool": build_stool,
    "tavern_stool_tall": lambda: build_stool(0.76),
    "tavern_barrel": build_barrel,
    "tavern_bar": build_bar,
    "tavern_backbar": build_backbar,
    "tavern_tankard": lambda: build_tankard(False),
    "tavern_tankard_wood": lambda: build_tankard(True),
    "tavern_candle": build_candle,
    "tavern_chandelier": build_chandelier,
    "tavern_sconce": build_sconce,
    "tavern_lantern": build_lantern,
    "tavern_door": build_door,
}


# ─── Textures ───────────────────────────────────────────────────────────────

def build_textures(flint: Path):
    from PIL import Image

    TEX_DIR.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        for name in TEXTURE_SETS:
            spec = DEMO / "specs" / f"tavern_{name}.procgen.toml"
            stem = Path(tmp) / f"tavern_{name}.png"
            subprocess.run([str(flint), "gen", str(spec), "-o", str(stem)], check=True,
                           stdout=subprocess.DEVNULL)
            color = Image.open(Path(tmp) / f"tavern_{name}_color.png").convert("RGB")
            normal = Image.open(Path(tmp) / f"tavern_{name}_normal.png").convert("RGB")
            rough = Image.open(Path(tmp) / f"tavern_{name}_roughness.png").convert("L")
            metal = Image.new("L", rough.size, 255 if name in METAL_SETS else 0)
            mr = Image.merge("RGB", (Image.new("L", rough.size, 255), rough, metal))
            color.save(TEX_DIR / f"tavern_{name}_color.jpg", quality=90, subsampling=0)
            normal.save(TEX_DIR / f"tavern_{name}_normal.jpg", quality=92, subsampling=0)
            mr.save(TEX_DIR / f"tavern_{name}_mr.jpg", quality=90, subsampling=0)
            print(f"  textures/tavern/tavern_{name}_{{color,normal,mr}}.jpg  {color.size[0]}px")


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--textures", action="store_true", help="run flint gen on the tavern specs first")
    ap.add_argument("--flint", type=Path, help="flint binary (default target/release/flint)")
    ap.add_argument("--only", nargs="*", help="build only these models")
    args = ap.parse_args()
    if args.textures:
        flint = args.flint or next((p for p in (REPO / "target/release/flint.exe", REPO / "target/release/flint",
                                                 REPO / "target/debug/flint.exe", REPO / "target/debug/flint") if p.exists()), None)
        if flint is None:
            sys.exit("no flint binary: cargo build --release --bin flint, or pass --flint")
        print("textures:")
        build_textures(flint)
    print("models:")
    MODEL_DIR.mkdir(parents=True, exist_ok=True)
    for name, fn in MODELS.items():
        if args.only and name not in args.only:
            continue
        write_glb(fn(), MODEL_DIR / f"{name}.glb", name)


if __name__ == "__main__":
    main()
