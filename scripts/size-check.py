#!/usr/bin/env python3
"""What the tracked tree may carry (#759, ADR 0122). `scripts/ci.sh` runs it.

    python3 scripts/size-check.py            # check; exit 1 on a breach
    python3 scripts/size-check.py --report   # the sizes too, by area

Every game carries this whole tree as `engine/` (`git subtree add`), so every
byte here is a byte in every game clone, worktree and CI checkout. It checks
the files `git ls-files` lists, as they are on disk:

1. **No binary is tracked twice.** Two files with the same bytes are one
   file: point the second user at the first (a scene names a path as
   `../models/x.glb`; a golden case says `golden = "<other>.png"`).
   `DUPLICATE_EXCEPTIONS` names the copies kept on purpose, with why.
2. **Textures are capped**: at most `TEXTURE_SIDE` texels a side and
   `TEXTURE_BYTES` encoded, loose or inside a `.glb`. Book images and
   golden renders are pictures, not textures, and are not held to it.
   `tools/assets/cap_textures.py` brings one under, as the renderer's own
   mip filter would.
3. **No file is over `FILE_BYTES`**, but those `FILE_EXCEPTIONS` names.
4. **The tree is under `TREE_BYTES`**, and each area under its budget in
   `AREAS`. A breach means the budget in ADR 0122 is to be argued with,
   not raised in passing.
5. **A book PNG has no alpha channel** (#1276), but those `ALPHA_EXCEPTIONS`
   names. A render or screenshot is saved RGBA with every pixel opaque;
   `tools/assets/pack_book_images.py` drops the channel, same pixels.

Standard library only: CI runs it before anything is installed.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import struct
import subprocess
import sys
import tomllib
from collections import defaultdict
from pathlib import Path

MB = 1_000_000

TEXTURE_SIDE = 2048
TEXTURE_BYTES = 3 * MB
FILE_BYTES = 8 * MB
# Over FILE_BYTES on purpose, each with why. Add one only with a reason a
# reviewer would accept; ADR 0122 lists them.
FILE_EXCEPTIONS: dict[str, str] = {}
# Copies kept on purpose, each group with why (ADR 0122 lists them).
DUPLICATE_EXCEPTIONS: dict[frozenset, str] = {
    # The bartender and patrons of the showcase game (#984): a game in the
    # checkout searches its own project, and the frozen fixtures load their
    # own copy so a change to the game never moves a golden. 2.4 MB.
    frozenset({
        "showcase_game/models/YBotWithIdle.glb",
        "tests/fixtures/scenes/models/YBotWithIdle.glb",
    }): "showcase_game plays it inside its own project; the fixtures stay frozen apart from the game",
}
# The frozen frame captures (#1087): one scene with and without a lantern,
# and a pass the lantern never reached (two cascades, bloom) is the same bytes
# in both. That sameness is what `flint frame diff` is tested on, and a
# capture's manifest names its images inside its own folder. 2.5 KB.
for _image in (
    "03-shadow_cascade_1.exr", "04-shadow_cascade_2.exr", "12-bloom_mip_0-threshold.exr",
    "12-bloom_mip_0-up.exr", "12-bloom_mip_1-down.exr",
):
    DUPLICATE_EXCEPTIONS[frozenset({
        f"tests/fixtures/frames/one_crate.flintframe/targets/{_image}",
        f"tests/fixtures/frames/two_crates.flintframe/targets/{_image}",
    })] = "a frame capture holds its own images; the pass was untouched, so its image is too"
# The budgets (ADR 0122): what the tree measured when they were set, with a
# tenth or so to grow into. 2026-11-14 (#759): 168.8 MB under 185. Reset
# 2026-11-20 when the examples left for flint-examples (#984, ADR 0131):
# 158.0 MB.
TREE_BYTES = 175 * MB
# What a game's engine/ carries, by part. The first area whose prefix
# matches a path takes it; "engine" is everything else.
AREAS = [
    ("book", ("docs/",), 78 * MB),  # 70.2
    ("tests", ("tests/", "crates/flint-cli/tests/golden/"), 50 * MB),  # 45.7 (#984: the 19.2 MB the fixtures borrowed)
    ("showcase_game", ("showcase_game/",), 18 * MB),  # 16.2 (#984: its own YBot)
    ("engine", ("",), 23 * MB),  # 16.1; 20.5 at #1303, all source: crates, schemas, styles, scripts, tools
]

BINARY = {
    ".glb", ".gltf", ".bin", ".png", ".jpg", ".jpeg", ".webp", ".gif", ".bmp",
    ".tga", ".tif", ".tiff", ".hdr", ".exr", ".ktx2", ".dds", ".ogg", ".wav",
    ".mp3", ".flac", ".mp4", ".webm", ".ttf", ".otf", ".woff", ".woff2", ".ico",
    ".zip", ".gz", ".apk", ".wasm", ".exe", ".dll", ".pdf",
}
IMAGES = {".png", ".jpg", ".jpeg", ".webp", ".gif", ".bmp", ".tga", ".hdr"}
NOT_TEXTURES = ("docs/", "crates/flint-cli/tests/golden/")
# Book PNGs whose transparency the page needs, each with why (#1276).
ALPHA_EXCEPTIONS: dict[str, str] = {}

def tracked() -> tuple[list[Path], set[str]]:
    """The tracked files on disk, and every tracked path (a sparse checkout
    writes out only some)."""
    out = subprocess.run(
        ["git", "ls-files", "-z"], capture_output=True, check=True
    ).stdout.decode("utf-8")
    names = {p for p in out.split("\0") if p}
    return [Path(p) for p in sorted(names) if Path(p).is_file()], names


def png_size(b: bytes):
    if b[:8] == b"\x89PNG\r\n\x1a\n" and b[12:16] == b"IHDR":
        return struct.unpack(">II", b[16:24])
    return None


def jpeg_size(b: bytes):
    if b[:2] != b"\xff\xd8":
        return None
    i = 2
    while i + 9 < len(b):
        if b[i] != 0xFF:
            i += 1
            continue
        marker = b[i + 1]
        if marker in (0xD8, 0x01) or 0xD0 <= marker <= 0xD7:
            i += 2
            continue
        length = struct.unpack(">H", b[i + 2 : i + 4])[0]
        if 0xC0 <= marker <= 0xCF and marker not in (0xC4, 0xC8, 0xCC):
            h, w = struct.unpack(">HH", b[i + 5 : i + 9])
            return w, h
        i += 2 + length
    return None


def image_size(b: bytes):
    return png_size(b) or jpeg_size(b)


def glb_images(b: bytes):
    """(index, name, bytes) of every image embedded in a GLB."""
    if b[:4] != b"glTF":
        return
    off, js, binc = 12, None, b""
    while off + 8 <= len(b):
        clen, ctype = struct.unpack_from("<I4s", b, off)
        chunk = b[off + 8 : off + 8 + clen]
        if ctype == b"JSON":
            js = json.loads(chunk)
        elif ctype == b"BIN\0":
            binc = chunk
        off += 8 + clen
    for i, im in enumerate((js or {}).get("images", [])):
        if "bufferView" in im:
            v = js["bufferViews"][im["bufferView"]]
            o = v.get("byteOffset", 0)
            yield i, im.get("name", ""), binc[o : o + v["byteLength"]]


def area_of(path: str) -> str:
    for name, prefixes, _ in AREAS:
        if any(path.startswith(p) for p in prefixes):
            return name
    return "engine"


def check(files: list[Path], names: set[str], report: bool) -> list[str]:
    problems: list[str] = []
    by_hash: dict[str, list[str]] = defaultdict(list)
    sizes: dict[str, int] = defaultdict(int)
    total = 0
    for f in files:
        p = f.as_posix()
        n = f.stat().st_size
        total += n
        sizes[area_of(p)] += n
        ext = f.suffix.lower()
        if n > FILE_BYTES and p not in FILE_EXCEPTIONS:
            problems.append(f"{p}: {n / MB:.1f} MB, over the {FILE_BYTES // MB} MB a file may be")
        if ext not in BINARY:
            continue
        data = f.read_bytes()
        by_hash[hashlib.sha256(data).hexdigest()].append(p)
        # colour type 4 or 6: grey or RGB with alpha
        if p.startswith("docs/") and png_size(data) and data[25] in (4, 6) and p not in ALPHA_EXCEPTIONS:
            problems.append(
                f"{p}: a book PNG with an alpha channel; a render's is 255 everywhere, about 8% of the file "
                f"(python tools/assets/pack_book_images.py {p}: same pixels; a needed alpha: ALPHA_EXCEPTIONS)"
            )
        if p.startswith(NOT_TEXTURES):
            continue
        textures = []
        if ext in IMAGES:
            textures.append((p, data))
        elif ext == ".glb":
            textures += [(f"{p} image {i} {name!r}", img) for i, name, img in glb_images(data)]
        for where, img in textures:
            size = image_size(img)
            if size and max(size) > TEXTURE_SIDE:
                problems.append(
                    f"{where}: {size[0]}x{size[1]}, over {TEXTURE_SIDE} a side "
                    f"(python tools/assets/cap_textures.py)"
                )
            if len(img) > TEXTURE_BYTES:
                problems.append(
                    f"{where}: {len(img) / MB:.1f} MB, over the {TEXTURE_BYTES / MB:.0f} MB a texture may be "
                    f"(python tools/assets/cap_textures.py; a colour map as JPEG)"
                )
    for paths in by_hash.values():
        if len(paths) > 1 and frozenset(paths) not in DUPLICATE_EXCEPTIONS:
            problems.append(
                "the same bytes are tracked " + str(len(paths)) + " times: " + ", ".join(sorted(paths))
                + " (keep one and point the others' users at it)"
            )
    on_disk = {f.as_posix() for f in files}
    for missing in sorted(set(FILE_EXCEPTIONS) - names):
        problems.append(f"FILE_EXCEPTIONS names {missing}, which isn't tracked: drop it")
    groups = {frozenset(paths) for paths in by_hash.values()}
    for group in DUPLICATE_EXCEPTIONS:
        # a sparse checkout may write out only one of the pair
        if group not in groups and group <= on_disk:
            problems.append(f"DUPLICATE_EXCEPTIONS names {', '.join(sorted(group))}, no longer one file twice: drop it")
    if total > TREE_BYTES:
        problems.append(f"the tree is {total / MB:.1f} MB, over its {TREE_BYTES // MB} MB budget (ADR 0122)")
    for name, _, budget in AREAS:
        if sizes[name] > budget:
            problems.append(f"{name} is {sizes[name] / MB:.1f} MB, over its {budget // MB} MB budget (ADR 0122)")
    if report:
        print(f"size-check: {len(files)} files, {total / MB:.1f} MB of {TREE_BYTES // MB} MB")
        for name, _, budget in AREAS:
            print(f"  {name:14} {sizes[name] / MB:7.1f} MB of {budget // MB:4d} MB")
    return problems


def main() -> int:
    # The engine's root, wherever it is run from: in a game that is engine/,
    # and `git ls-files` there lists the engine's files only
    os.chdir(Path(__file__).resolve().parent.parent)
    report = "--report" in sys.argv
    files, names = tracked()
    problems = check(files, names, report)
    for p in problems:
        print(f"size-check: {p}", file=sys.stderr)
    if problems:
        print(f"size-check: {len(problems)} problem(s); the rules are in scripts/size-check.py and ADR 0122", file=sys.stderr)
        return 1
    print("size-check: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
