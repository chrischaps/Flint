"""Pack the book's PNGs smaller with the same pixels (#1276, ADR 0122).

    python tools/assets/pack_book_images.py                 # every tracked PNG under docs/
    python tools/assets/pack_book_images.py docs/book/src/images/x.png
    python tools/assets/pack_book_images.py --dry-run

A render saved by `flint render` or a screenshot is RGBA, and a picture of
a frame has every pixel opaque, so a quarter of its bytes before compression
say nothing. For each PNG this tries, losslessly:

- dropping an alpha channel that is 255 everywhere (RGBA to RGB, LA to L);
- a palette, when the image has 256 colours or fewer;
- Pillow's `optimize=True` (its slowest zlib settings), for each of those.

It decodes each candidate and keeps the smallest whose pixels match the
original exactly (compared as RGBA), and writes it only when it is smaller
than the file. So a file it touches shows the same picture, and a file it
can't shrink is left byte for byte. A PNG carrying any chunk but IHDR,
PLTE, tRNS, IDAT and IEND (a colour profile, gamma, text) is skipped, since
Pillow would drop the chunk.

This is lossless only. A render that needs to be smaller still goes to a
palette by eye (`quantize(colors=128)`, as most of the book's sheets are) or
to JPEG; that is a choice about the picture, made per image.

`scripts/size-check.py` refuses a PNG under `docs/` that still carries alpha
(unless `ALPHA_EXCEPTIONS` names it) and points here.

Needs Pillow (`tools/requirements.txt`).
"""

from __future__ import annotations

import argparse
import io
import struct
import subprocess
import sys
from pathlib import Path

from PIL import Image

PLAIN_CHUNKS = {b"IHDR", b"PLTE", b"tRNS", b"IDAT", b"IEND"}


def chunk_types(data: bytes) -> set[bytes]:
    types, i = set(), 8
    while i + 8 <= len(data):
        n, t = struct.unpack(">I4s", data[i : i + 8])
        types.add(t)
        i += 12 + n
    return types


def encode(im: Image.Image) -> bytes:
    buf = io.BytesIO()
    im.save(buf, "PNG", optimize=True)
    return buf.getvalue()


def rgba(im: Image.Image) -> bytes:
    return im.convert("RGBA").tobytes()


def candidates(im: Image.Image):
    """Images that may show the same pixels as `im` in fewer bytes."""
    yield im
    base = im
    if im.mode in ("RGBA", "LA") and im.getchannel("A").getextrema() == (255, 255):
        base = im.convert("RGB" if im.mode == "RGBA" else "L")
        yield base
    if base.mode in ("RGB", "RGBA") and base.getcolors(256) is not None:
        # quantize() maps exactly when the colours fit; the pixel check
        # below drops it if not.
        method = Image.Quantize.FASTOCTREE if base.mode == "RGBA" else Image.Quantize.MEDIANCUT
        yield base.quantize(colors=256, method=method, dither=Image.Dither.NONE)


def pack(path: Path) -> tuple[bytes, bytes, str]:
    """(old bytes, the smallest same-pixel bytes, a note) for one PNG."""
    data = path.read_bytes()
    extra = chunk_types(data) - PLAIN_CHUNKS
    if extra:
        names = ", ".join(sorted(t.decode("latin-1") for t in extra))
        return data, data, f"skipped: carries {names}"
    with Image.open(io.BytesIO(data)) as src:
        src.load()
        want = rgba(src)
        best, mode = data, src.mode
        for cand in candidates(src):
            out = encode(cand)
            if len(out) >= len(best):
                continue
            with Image.open(io.BytesIO(out)) as back:
                if rgba(back) != want:
                    continue
            best, mode = out, cand.mode
    if len(best) < len(data):
        return data, best, f"{src.mode} -> {mode}"
    return data, data, "already packed"


def tracked_pngs() -> list[Path]:
    out = subprocess.run(
        ["git", "ls-files", "-z", "--", "docs/*.png"], capture_output=True, check=True
    ).stdout.decode("utf-8")
    return [Path(p) for p in sorted(out.split("\0")) if p and Path(p).is_file()]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("paths", nargs="*", type=Path, help="PNGs to pack (default: every tracked PNG under docs/)")
    ap.add_argument("--dry-run", action="store_true", help="say what would change, write nothing")
    args = ap.parse_args()
    paths = args.paths or tracked_pngs()
    total_old = total_new = changed = 0
    for p in paths:
        before, after, note = pack(p)
        old, new = len(before), len(after)
        total_old += old
        total_new += new
        if new < old:
            changed += 1
            if not args.dry_run:
                p.write_bytes(after)
            print(f"{p.as_posix()}: {old / 1e6:.2f} -> {new / 1e6:.2f} MB ({note})")
        elif note.startswith("skipped"):
            print(f"{p.as_posix()}: {note}", file=sys.stderr)
    verb = "would pack" if args.dry_run else "packed"
    print(
        f"{verb} {changed} of {len(paths)} PNGs: {total_old / 1e6:.2f} -> {total_new / 1e6:.2f} MB, same pixels"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
