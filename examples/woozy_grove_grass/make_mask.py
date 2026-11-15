"""The grass mask of the Woozy Grove's meadow variant (#252).

    python examples/woozy_grove_grass/make_mask.py

Writes mask.png beside this file: 512 px over the 60 m square from (-30, -30) to
(30, 30), pixel (0, 0) at the minimum corner and rows running toward +Z, as the
`grass_ground` component reads it. White is full meadow, black bare ground.

The meadow is worn where people have walked: round the bench, along the path in
from the north and out to each of the three pockets. It thins at every trunk,
where roots and shade keep it short, and in the hush, where the cypress drop
their needles. Tree positions are the grove's own (examples/woozy_grove.scene.toml).
"""

from pathlib import Path

import numpy as np
from PIL import Image

SIZE = 512
LO, HI = -30.0, 30.0

# Every trunk in the grove, (x, z).
TREES = [
    (13.2, 1.6), (15.2, 7.2), (13.6, 12.4), (18.4, -2.0), (20.0, 4.0), (19.2, 10.4),
    (17.2, 15.6), (23.6, 7.6), (24.0, -3.2), (-9.6, 10.4), (-16.8, 7.6), (-20.4, 15.6),
    (-12.0, 18.8), (-13.2, 12.8), (-7.2, 15.6), (-16.4, 14.4), (-21.2, 9.2),
    (-12.8, -9.2), (-14.8, -12.0), (-16.4, -7.2), (-18.4, -10.4), (-15.2, -14.8),
    (-20.0, -13.6), (-20.4, -6.8), (-17.2, -12.4), (-12.8, -14.4), (-21.2, -10.0),
    (8.8, -6.8), (2.4, 13.6), (-6.8, -5.2), (6.0, 8.4), (-3.2, -9.2), (7.6, -12.0),
    (-2.0, 18.0), (4.8, -9.6), (-7.6, 6.0), (10.0, 2.8), (-9.6, -0.8),
]
BENCH = (-0.3, 3.4)

# Footpaths, as control points a walker would take: in from the north edge where
# you start, then out to the hush (+X), the drowse and the shiver.
PATHS = [
    [(1.2, -27.0), (1.4, -17.0), (0.2, -8.0), (-0.6, -1.0), BENCH],
    [BENCH, (5.0, 4.6), (9.5, 5.4), (14.0, 5.0)],
    [BENCH, (-4.0, 7.2), (-8.0, 9.0), (-11.5, 11.5)],
    [BENCH, (-4.2, 0.8), (-8.4, -4.0), (-11.6, -8.0)],
]


def smoothstep(e0, e1, x):
    t = np.clip((x - e0) / (e1 - e0), 0.0, 1.0)
    return t * t * (3.0 - 2.0 * t)


def value_noise(x, z, scale, seed):
    """Smooth value noise at `scale` metres, from a fixed lattice."""
    rng = np.random.default_rng(seed)
    n = int((HI - LO) / scale) + 3
    lattice = rng.random((n, n))
    fx, fz = (x - LO) / scale, (z - LO) / scale
    ix, iz = np.floor(fx).astype(int), np.floor(fz).astype(int)
    tx, tz = fx - ix, fz - iz
    tx, tz = tx * tx * (3 - 2 * tx), tz * tz * (3 - 2 * tz)
    a = lattice[iz, ix] * (1 - tx) + lattice[iz, ix + 1] * tx
    b = lattice[iz + 1, ix] * (1 - tx) + lattice[iz + 1, ix + 1] * tx
    return a * (1 - tz) + b * tz


def catmull_rom(points, per_segment=24):
    p = [points[0]] + list(points) + [points[-1]]
    out = []
    for i in range(1, len(p) - 2):
        p0, p1, p2, p3 = (np.array(q) for q in p[i - 1 : i + 3])
        for t in np.linspace(0.0, 1.0, per_segment, endpoint=False):
            out.append(
                0.5
                * (
                    2 * p1
                    + (-p0 + p2) * t
                    + (2 * p0 - 5 * p1 + 4 * p2 - p3) * t * t
                    + (-p0 + 3 * p1 - 3 * p2 + p3) * t ** 3
                )
            )
    out.append(np.array(points[-1]))
    return np.array(out)


def distance_to_polyline(x, z, line):
    best = np.full(x.shape, np.inf)
    for a, b in zip(line[:-1], line[1:]):
        ab = b - a
        t = np.clip(((x - a[0]) * ab[0] + (z - a[1]) * ab[1]) / max(ab @ ab, 1e-9), 0, 1)
        d = np.hypot(x - (a[0] + t * ab[0]), z - (a[1] + t * ab[1]))
        best = np.minimum(best, d)
    return best


def main():
    centres = LO + (np.arange(SIZE) + 0.5) * (HI - LO) / SIZE
    x, z = np.meshgrid(centres, centres)  # rows run toward +Z

    # Clumps and gaps, so the meadow isn't a carpet.
    clumps = (
        0.55 * value_noise(x, z, 6.0, 11)
        + 0.3 * value_noise(x, z, 2.5, 12)
        + 0.15 * value_noise(x, z, 1.0, 14)
    )
    density = 0.5 + 0.5 * smoothstep(0.2, 0.75, clumps)

    # Worn round the bench: bare where you'd stand, short a little further out.
    d_bench = np.hypot(x - BENCH[0], z - BENCH[1])
    density *= 0.15 + 0.85 * smoothstep(1.4, 4.2, d_bench)

    # Footpaths, a little wavering in width.
    waver = 0.25 * (value_noise(x, z, 2.0, 13) - 0.5)
    for path in PATHS:
        d = distance_to_polyline(x, z, catmull_rom(path)) + waver
        density *= 0.05 + 0.95 * smoothstep(0.35, 1.3, d)

    # Every trunk keeps a bare foot and a thinner ring of shade.
    for tx, tz in TREES:
        d = np.hypot(x - tx, z - tz)
        density *= 0.0 + smoothstep(0.5, 1.1, d) * (0.6 + 0.4 * smoothstep(1.1, 3.0, d))

    # The hush: needle litter under the cypress, so the meadow thins.
    hush = smoothstep(9.0, 13.5, x) * (1 - smoothstep(16.0, 22.0, np.abs(z - 5.5)))
    density *= 1.0 - 0.35 * hush

    image = Image.fromarray((np.clip(density, 0, 1) * 255).round().astype(np.uint8), "L")
    out = Path(__file__).with_name("mask.png")
    image.save(out)
    print(f"wrote {out} ({SIZE}x{SIZE})")


if __name__ == "__main__":
    main()
