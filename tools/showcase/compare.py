"""Compare two captures of the same take, frame by frame (#435, #341).

    python tools/showcase/compare.py <take> <take.again> [--json report.json]

Frames pair by name. For each pair: how many pixels differ, the largest
difference in any channel (in 8-bit levels), and the mean. A take is
deterministic when every pair is byte-identical; issue #341 found
`--frame-step` captures differing by one level in a handful of pixels, so
the report counts frames by their worst difference too, which tells a
one-level wobble from a real divergence (a wrong glyph is tens of levels
over a whole cell).

Needs Pillow and numpy.
"""

import argparse
import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image


def compare_folders(a, b):
    a, b = Path(a), Path(b)
    names_a = {p.name for p in a.glob("frame_*.png")}
    names_b = {p.name for p in b.glob("frame_*.png")}
    common = sorted(names_a & names_b)
    differing = []
    by_level = {}
    worst = 0
    for name in common:
        x = np.asarray(Image.open(a / name).convert("RGB"), dtype=np.int16)
        y = np.asarray(Image.open(b / name).convert("RGB"), dtype=np.int16)
        if x.shape != y.shape:
            differing.append({"frame": name, "shape": [list(x.shape), list(y.shape)]})
            worst = 255
            continue
        d = np.abs(x - y).max(axis=2)
        n = int((d > 0).sum())
        if n == 0:
            continue
        m = int(d.max())
        worst = max(worst, m)
        by_level[m] = by_level.get(m, 0) + 1
        differing.append({"frame": name, "pixels": n, "max_level": m,
                          "fraction": round(n / d.size, 8)})
    only = sorted(names_a ^ names_b)
    identical = len(common) - len(differing)
    if not differing and not only:
        summary = f"identical: all {len(common)} frames byte for byte"
    else:
        worst_pixels = max((f.get("pixels", 0) for f in differing), default=0)
        summary = (
            f"{identical}/{len(common)} frames identical; {len(differing)} differ "
            f"(worst {worst} levels, at most {worst_pixels} pixels in a frame)"
            + (f"; {len(only)} frames in one run only" if only else "")
        )
    return {
        "a": str(a),
        "b": str(b),
        "frames": len(common),
        "identical": identical,
        "differing": differing,
        "frames_by_max_level": {str(k): v for k, v in sorted(by_level.items())},
        "only_in_one": only,
        "max_level": worst,
        "summary": summary,
    }


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("a")
    ap.add_argument("b")
    ap.add_argument("--json", help="write the report here")
    args = ap.parse_args()
    report = compare_folders(args.a, args.b)
    print(report["summary"])
    for f in report["differing"][:20]:
        print(f"  {f}")
    if args.json:
        Path(args.json).write_text(json.dumps(report, indent=2), encoding="utf-8")
    sys.exit(0 if report["max_level"] == 0 and not report["only_in_one"] else 1)


if __name__ == "__main__":
    main()
