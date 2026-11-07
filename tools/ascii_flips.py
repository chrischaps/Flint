"""Count ASCII glyph flips across a captured frame sequence (#277).

    python tools/ascii_flips.py out/on_*.png [--cell 8] [--json result.json]

Each frame is cut into the ASCII style's grid (cells `--cell` px square,
centred as `grid_origin` centres it) and each cell's glyph is read back as
its ink mask: with the `source` and `raw` colour modes at the default
background a glyph's ink is bright and its ground dim, so a pixel is ink
when its brightest channel passes `--ink` (0-255). Two numbers per run:

- flips: cells whose glyph differs from the frame before, per frame. Real
  motion flips glyphs too; this is the total.
- reversals: cells whose glyph changed and is now back to what it was two
  frames ago (A, B, A). Motion rarely does that in two frames; shimmer
  does little else, so this is the shimmer itself.

Frames are taken in name order. Needs Pillow and numpy.
"""

import argparse
import glob
import json
import sys

import numpy as np
from PIL import Image


def glyph_masks(path, cell, ink):
    img = np.asarray(Image.open(path).convert("RGB"))
    h, w, _ = img.shape
    ox, oy = (w % cell) // 2, (h % cell) // 2
    cols, rows = (w - ox) // cell, (h - oy) // cell
    core = img[oy : oy + rows * cell, ox : ox + cols * cell].max(axis=2) > ink
    # (rows, cols, cell*cell) bit patterns, packed to bytes per cell
    blocks = core.reshape(rows, cell, cols, cell).transpose(0, 2, 1, 3)
    return np.packbits(blocks.reshape(rows, cols, cell * cell), axis=2)


def measure(paths, cell, ink):
    masks = [glyph_masks(p, cell, ink) for p in paths]
    cells = masks[0].shape[0] * masks[0].shape[1]
    flips, reversals = [], []
    for i in range(1, len(masks)):
        changed = np.any(masks[i] != masks[i - 1], axis=2)
        flips.append(int(changed.sum()))
        if i >= 2:
            back = np.all(masks[i] == masks[i - 2], axis=2) & changed
            reversals.append(int(back.sum()))
    return {
        "frames": len(masks),
        "cells": cells,
        "flips_per_frame": float(np.mean(flips)) if flips else 0.0,
        "flips_pct": 100.0 * float(np.mean(flips)) / cells if flips else 0.0,
        "reversals_per_frame": float(np.mean(reversals)) if reversals else 0.0,
        "reversals_pct": 100.0 * float(np.mean(reversals)) / cells if reversals else 0.0,
        "flips": flips,
        "reversals": reversals,
    }


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("frames", nargs="+", help="PNG frames (globs are expanded)")
    ap.add_argument("--cell", type=int, default=8, help="cell size in px (default 8)")
    ap.add_argument("--ink", type=int, default=128, help="ink threshold 0-255 (default 128)")
    ap.add_argument("--json", help="write the full result here")
    args = ap.parse_args()
    paths = sorted(p for pattern in args.frames for p in glob.glob(pattern) or [pattern])
    if len(paths) < 3:
        sys.exit("need at least 3 frames")
    r = measure(paths, args.cell, args.ink)
    print(
        f"{r['frames']} frames, {r['cells']} cells: "
        f"{r['flips_per_frame']:.1f} flips/frame ({r['flips_pct']:.2f}%), "
        f"{r['reversals_per_frame']:.1f} reversals/frame ({r['reversals_pct']:.2f}%)"
    )
    if args.json:
        with open(args.json, "w") as f:
            json.dump(r, f, indent=1)


if __name__ == "__main__":
    main()
