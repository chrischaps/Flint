"""Woozy grass, judged by eye (#252): every variant tried, rendered the same way.

    cargo build --release --bin flint
    python examples/woozy_grove_grass/experiment.py              # all of it, about 4 minutes
    python examples/woozy_grove_grass/experiment.py --only comb  # one variant

For each variant of the meadow in examples/woozy_grove_grass.scene.toml it writes a
4-second GIF from three places (the north path, inside the hush, inside the
shiver) to renders/woozy_grass/, and prints how the grass moves: of the frame
pairs at 12 fps, how many change in a crop that holds only grass, and by how much.

The variants are written beside the scene as .woozy_grass_<name>.scene.toml, so
every path in it resolves as the scene's own, and removed afterwards.
"""

import argparse
import re
import subprocess
import sys
from pathlib import Path

import numpy as np
from PIL import Image

ROOT = Path(__file__).resolve().parents[2]
SCENE = ROOT / "examples" / "woozy_grove_grass.scene.toml"
OUT = ROOT / "renders" / "woozy_grass"
FLINT = ROOT / "target" / "release" / ("flint.exe" if sys.platform == "win32" else "flint")

# name -> what the meadow's wooze lines become, and what to look at.
VARIANTS = {
    "wall": ({"grass.wooze": "false"}, "the grass as it was: sways on the wall clock"),
    "clock": (
        {"grass.wooze": "true", "grass.wooze_amplitude": "0.0"},
        "the field's clock only: holds and glides, no comb",
    ),
    "comb": ({}, "as authored: the clock, and combed 0.14 m each beat"),
    "deep": (
        {"grass.wooze_amplitude": "0.3", "grass.wooze_wavelength": "1.2"},
        "a deep, tight comb: 0.3 m, 1.2 m patches",
    ),
}

# name -> flint render camera flags. The grass clock is read where the camera is.
SHOTS = {
    "north": [],  # the scene's [camera]: the north path, into the clearing
    # Standing among the cypress, looking back toward the clearing.
    "hush": ["--target=11,0.3,4.5", "--distance", "5.5", "--pitch", "15", "--yaw", "90"],
    # Standing among the thorns, looking out along their path.
    "shiver": ["--target=-7.5,0.3,-12.5", "--distance", "5", "--pitch", "14", "--yaw", "270"],
}

# The measuring pass: smaller, and with the shading boil off, since the boil
# redraws every pixel 12 times a second and would move a crop of bare ground.
MEASURE = ["--kuwahara-boil", "0", "--contour-boil", "0"]
# A control for the measure: the same shots with no grass at all.
CONTROL = {"grass.enabled": "false"}

WIDTH, HEIGHT = 960, 540
SPAN, FPS = "0..4", "12"
# A crop with grass and nothing else in it: the bottom fifth, away from the
# bench and the trunks in every shot.
CROP = (0.05, 0.82, 0.95, 0.98)


def variant_scene(name: str, changes: dict) -> Path:
    text = SCENE.read_text(encoding="utf-8")
    for key, value in changes.items():
        line = re.compile(rf'^"{re.escape(key)}" = [^\n#]*', re.M)
        text, n = line.subn(f'"{key}" = {value} ', text)
        if n == 0:
            # Not authored: add it to the meadow's grass_ground.
            anchor = "scale_by_mask = true\n"
            if text.count(anchor) != 1:
                raise SystemExit(f"can't place {key} in {SCENE.name}")
            text = text.replace(anchor, f'{anchor}"{key}" = {value}\n')
        elif n != 1:
            raise SystemExit(f"{key} found {n} times in {SCENE.name}")
    path = SCENE.with_name(f".woozy_grass_{name}.scene.toml")
    path.write_text(text, encoding="utf-8")
    return path


def render(scene: Path, out: Path, camera: list, measure: bool = False) -> None:
    width, height, extra = (WIDTH // 2, HEIGHT // 2, MEASURE) if measure else (WIDTH, HEIGHT, [])
    command = [
        str(FLINT), "render", str(scene), "--schemas", str(ROOT / "schemas"),
        "--time", SPAN, "--fps", FPS, "--width", str(width), "--height", str(height),
        "--output", str(out), *camera, *extra,
    ]
    done = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
    if done.returncode != 0:
        raise SystemExit(f"flint render failed for {out.name}:\n{done.stderr[-2000:]}")


def motion(gif: Path):
    """Frame pairs that change in the grass crop, and their mean change (0-255)."""
    image = Image.open(gif)
    frames = []
    for i in range(image.n_frames):
        image.seek(i)
        rgb = np.asarray(image.convert("RGB"), dtype=np.float32)
        h, w, _ = rgb.shape
        x0, y0, x1, y1 = (int(CROP[0] * w), int(CROP[1] * h), int(CROP[2] * w), int(CROP[3] * h))
        frames.append(rgb[y0:y1, x0:x1])
    diffs = [float(np.abs(b - a).mean()) for a, b in zip(frames, frames[1:])]
    moving = [d for d in diffs if d > 0.05]
    return len(moving), len(diffs), (sum(moving) / len(moving) if moving else 0.0), diffs


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--only", choices=VARIANTS, help="one variant")
    parser.add_argument("--shot", choices=SHOTS, help="one place")
    parser.add_argument("--measure-only", action="store_true", help="skip the GIFs to watch")
    args = parser.parse_args()
    if not FLINT.exists():
        raise SystemExit(f"build it first: cargo build --release --bin flint ({FLINT})")
    (OUT / "measure").mkdir(parents=True, exist_ok=True)

    print("Frame pairs at 12 fps whose grass crop changes (# moves, . holds), boil off:\n")
    runs = {"none": (CONTROL, "the control: no grass, so nothing in the crop should move")}
    runs.update(VARIANTS)
    for name, (changes, what) in runs.items():
        if args.only and name not in (args.only, "none"):
            continue
        scene = variant_scene(name, changes)
        try:
            for shot, camera in SHOTS.items():
                if args.shot and shot != args.shot:
                    continue
                if name != "none" and not args.measure_only:
                    render(scene, OUT / f"{name}_{shot}.gif", camera)
                gif = OUT / "measure" / f"{name}_{shot}.gif"
                render(scene, gif, camera, measure=True)
                changed, pairs, mean, diffs = motion(gif)
                pattern = "".join("#" if d > 0.05 else "." for d in diffs)
                print(f"{name:6} {shot:7} {changed:2}/{pairs} move, mean {mean:5.2f}  {pattern}")
        finally:
            scene.unlink(missing_ok=True)
        print(f"       {what}\n")
    print(f"GIFs to watch in {OUT.relative_to(ROOT)}, measured ones in measure/")


if __name__ == "__main__":
    main()
