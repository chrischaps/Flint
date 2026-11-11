#!/usr/bin/env python3
"""The book's two pictures of a wooze look, made from a scene and nothing else (#219).

Plays the grove twice at a fixed step, once as a look used to be and once as it
is, and writes:

  <name>-strip.png  A crown at eighth-second moments, one row per look. What it
                    looks like from moment to moment, which is whether it's still
                    the same tree.
  <name>-slit.png   One row of pixels stacked over 4 s, one panel per look, with
                    time running down. How it moves: a lurch is a jag, a hold is a
                    straight line and a glide is a curve. It's the nearest thing
                    to watching that a still picture has.

Same scene, same binary, same bytes: the player steps a fixed 1/60 s.

    python tools/living-strip/living_strip.py --player target/release/flint-player

Needs a GPU and a window, numpy and Pillow.
"""
import argparse
import re
import shutil
import subprocess
import tempfile
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont

STEP = 1 / 60
START = 3.0  # past the load, with every clock long since running
SECONDS = 4.0
WIDTH, HEIGHT = 1280, 720
CROWN = (747, 0, 1280, 400)  # the near birch and the oak beside it
SLIT_ROW = 200
INK = (18, 16, 20)
PAPER = (232, 224, 208)

# The design's first guess, which the preset was a placeholder for until #219.
FIRST_GUESS = ("the first guess", {"amplitude": 1.0, "step": 0.25, "glide": 0.3, "jitter": 1.0}, 2)


def with_look(scene: str, knobs: dict, boil_on: int) -> str:
    """The scene with its global knobs written out, which beat its preset."""
    table = "".join(f"{k} = {v}\n" for k, v in knobs.items())
    scene, found = re.subn(r"\[wooze\]\npreset = \"[^\"]*\"\n", "[wooze]\n" + table, scene, count=1)
    assert found == 1, "no [wooze] preset line to replace"
    scene, found = re.subn(r"(?m)^boil_on = \d+.*$", f"boil_on = {boil_on}", scene, count=1)
    assert found == 1, "no boil_on line to replace"
    return scene


def capture(player: Path, scene: Path, schemas: Path, frames: Path) -> list:
    frames.mkdir(parents=True)
    every = f"{STEP:.10f}"
    subprocess.run(
        [str(player), str(scene), "--schemas", str(schemas), "--schemas", str(schemas / "examples"),
         "--width", str(WIDTH), "--height", str(HEIGHT), "--frame-step", every,
         "--screenshot", str(frames / "f.png"), "--screenshot-at", str(START),
         "--screenshot-every", every, "--exit-after", str(START + SECONDS)],
        check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    return sorted(frames.glob("*.png"))


def font(size: int):
    return ImageFont.load_default(size=size)


def strip(rows: list, out: Path, cells: int = 6, cell=(300, 225)) -> None:
    gap, label = 4, 26
    width = cells * cell[0] + (cells - 1) * gap
    sheet = Image.new("RGB", (width, len(rows) * (cell[1] + label)), INK)
    draw = ImageDraw.Draw(sheet)
    for r, (title, frames) in enumerate(rows):
        y = r * (cell[1] + label)
        draw.text((2, y + 5), title, fill=PAPER, font=font(15))
        for c in range(cells):
            i = 30 + c * 15  # every 1/8 s
            crop = Image.open(frames[i]).convert("RGB").crop(CROWN).resize(cell, Image.LANCZOS)
            x = c * (cell[0] + gap)
            sheet.paste(crop, (x, y + label))
            ImageDraw.Draw(sheet).text((x + 6, y + label + 4), f"{i * STEP:.3f} s", fill=PAPER, font=font(12))
    sheet.save(out)


def slit(rows: list, out: Path, row_height: int = 2) -> None:
    gap, label = 12, 26
    panels = []
    for _, frames in rows:
        lines = [np.asarray(Image.open(f).convert("RGB"))[SLIT_ROW, CROWN[0]:CROWN[2]] for f in frames[:240]]
        panels.append(Image.fromarray(np.stack(lines)).resize((CROWN[2] - CROWN[0], len(lines) * row_height), Image.NEAREST))
    w, h = panels[0].size
    sheet = Image.new("RGB", (len(panels) * w + (len(panels) - 1) * gap, h + label), INK)
    draw = ImageDraw.Draw(sheet)
    for i, ((title, _), panel) in enumerate(zip(rows, panels)):
        x = i * (w + gap)
        draw.text((x + 2, 5), f"{title}: 4 s, time runs down", fill=PAPER, font=font(15))
        sheet.paste(panel, (x, label))
    sheet.save(out)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--player", type=Path, default=Path("target/release/flint-player"))
    ap.add_argument("--scene", type=Path, default=Path("examples/woozy_grove.scene.toml"))
    ap.add_argument("--schemas", type=Path, default=Path("schemas"))
    ap.add_argument("--out", type=Path, default=Path("docs/book/src/images"))
    ap.add_argument("--name", default="linklater")
    args = ap.parse_args()

    text = args.scene.read_text(encoding="utf-8").replace("\r\n", "\n")
    before = args.scene.with_name("_living_strip_before.scene.toml")
    title, knobs, boil_on = FIRST_GUESS
    before.write_text(with_look(text, knobs, boil_on), encoding="utf-8")
    work = Path(tempfile.mkdtemp(prefix="living-strip-"))
    try:
        rows = [
            (title, capture(args.player, before, args.schemas, work / "before")),
            (args.name, capture(args.player, args.scene, args.schemas, work / "after")),
        ]
        strip(rows, args.out / f"{args.name}-strip.png")
        slit(rows, args.out / f"{args.name}-slit.png")
    finally:
        before.unlink(missing_ok=True)
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
