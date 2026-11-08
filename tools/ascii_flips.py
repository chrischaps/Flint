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

Under a fast camera both numbers count motion: a glyph that travels with
its surface into the next cell is a flip in two cells. `--track B` (#344)
follows the picture instead: for each block of B x B cells it finds the
shift, up to `--search` cells each way, at which last frame's glyphs best
match this frame's, and counts flips (and reversals, along two such
steps) along that shift. A surface whose glyphs travel unchanged then
counts nothing, however fast it moves; cells whose past fell off screen
are left out.

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


def shifted(a, dy, dx):
    """`a` moved so out[r, c] = a[r + dy, c + dx], and where that exists."""
    rows, cols = a.shape[:2]
    out = np.zeros_like(a)
    valid = np.zeros((rows, cols), dtype=bool)
    r0, r1 = max(0, -dy), min(rows, rows - dy)
    c0, c1 = max(0, -dx), min(cols, cols - dx)
    if r0 < r1 and c0 < c1:
        out[r0:r1, c0:c1] = a[r0 + dy : r1 + dy, c0 + dx : c1 + dx]
        valid[r0:r1, c0:c1] = True
    return out, valid


def block_shifts(cur, prev, block, search):
    """Per block of cells, the (dy, dx) where `prev` best matches `cur`.

    Returns per-cell dy, dx arrays (each cell takes its block's shift).
    Ties keep the smallest shift, so a still or featureless block stays put.
    """
    rows, cols = cur.shape[:2]
    br, bc = -(-rows // block), -(-cols // block)
    best = np.full((br, bc), np.inf)
    best_dy = np.zeros((br, bc), dtype=int)
    best_dx = np.zeros((br, bc), dtype=int)
    offsets = sorted(
        ((dy, dx) for dy in range(-search, search + 1) for dx in range(-search, search + 1)),
        key=lambda o: abs(o[0]) + abs(o[1]),
    )
    pad_r, pad_c = br * block - rows, bc * block - cols
    for dy, dx in offsets:
        moved, valid = shifted(prev, dy, dx)
        miss = (np.any(cur != moved, axis=2) & valid).astype(float)
        seen = valid.astype(float)
        miss = np.pad(miss, ((0, pad_r), (0, pad_c))).reshape(br, block, bc, block).sum(axis=(1, 3))
        seen = np.pad(seen, ((0, pad_r), (0, pad_c))).reshape(br, block, bc, block).sum(axis=(1, 3))
        # A shift must still see most of the block to count
        rate = np.where(seen >= 0.5 * block * block, miss / np.maximum(seen, 1), np.inf)
        better = rate < best
        best[better] = rate[better]
        best_dy[better] = dy
        best_dx[better] = dx
    up = lambda b: np.repeat(np.repeat(b, block, axis=0), block, axis=1)[:rows, :cols]
    return up(best_dy), up(best_dx)


def gather(a, dy, dx):
    """out[r, c] = a[r + dy[r, c], c + dx[r, c]], and where that exists."""
    rows, cols = a.shape[:2]
    r = np.arange(rows)[:, None] + dy
    c = np.arange(cols)[None, :] + dx
    valid = (r >= 0) & (r < rows) & (c >= 0) & (c < cols)
    return a[np.clip(r, 0, rows - 1), np.clip(c, 0, cols - 1)], valid


def measure_tracked(paths, cell, ink, block, search):
    masks = [glyph_masks(p, cell, ink) for p in paths]
    cells = masks[0].shape[0] * masks[0].shape[1]
    flips, reversals, compared, moves = [], [], [], []
    prev_shift = None
    for i in range(1, len(masks)):
        dy, dx = block_shifts(masks[i], masks[i - 1], block, search)
        past, valid = gather(masks[i - 1], dy, dx)
        changed = np.any(masks[i] != past, axis=2) & valid
        flips.append(int(changed.sum()))
        compared.append(int(valid.sum()))
        moves.append(float(np.mean(np.hypot(dy, dx))))
        if prev_shift is not None:
            # Two steps back: this frame's step, then last frame's from there
            pdy, pdx = prev_shift
            rows, cols = dy.shape
            r = np.clip(np.arange(rows)[:, None] + dy, 0, rows - 1)
            c = np.clip(np.arange(cols)[None, :] + dx, 0, cols - 1)
            past2, valid2 = gather(masks[i - 2], dy + pdy[r, c], dx + pdx[r, c])
            back = np.all(masks[i] == past2, axis=2) & changed & valid2
            reversals.append(int(back.sum()))
        prev_shift = (dy, dx)
    seen = float(np.mean(compared))
    return {
        "frames": len(masks),
        "cells": cells,
        "tracked": {"block": block, "search": search},
        "compared_per_frame": seen,
        "mean_shift_cells": float(np.mean(moves)),
        "flips_per_frame": float(np.mean(flips)),
        "flips_pct": 100.0 * float(np.mean(flips)) / seen,
        "reversals_per_frame": float(np.mean(reversals)),
        "reversals_pct": 100.0 * float(np.mean(reversals)) / seen,
        "flips": flips,
        "reversals": reversals,
    }


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("frames", nargs="+", help="PNG frames (globs are expanded)")
    ap.add_argument("--cell", type=int, default=8, help="cell size in px (default 8)")
    ap.add_argument("--ink", type=int, default=128, help="ink threshold 0-255 (default 128)")
    ap.add_argument(
        "--track",
        type=int,
        metavar="B",
        help="follow the picture in blocks of B x B cells (#344); flips counted along the motion",
    )
    ap.add_argument("--search", type=int, default=4, help="with --track: cells searched each way (default 4)")
    ap.add_argument("--json", help="write the full result here")
    args = ap.parse_args()
    paths = sorted(p for pattern in args.frames for p in glob.glob(pattern) or [pattern])
    if len(paths) < 3:
        sys.exit("need at least 3 frames")
    if args.track:
        r = measure_tracked(paths, args.cell, args.ink, args.track, args.search)
        print(f"tracked in {args.track}x{args.track} blocks, mean shift {r['mean_shift_cells']:.2f} cells/frame")
    else:
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
