"""The noise pose (#252's fourth target), tried by eye: a prototype, not the engine.

    cargo build --release --bin flint
    cargo build --release -p flint-procgen --example living_spike
    python examples/wooze_props/noise_pose.py              # every variant, about 10 minutes
    python examples/wooze_props/noise_pose.py --only pine  # the variants whose name starts so

`living_spike noise` (crates/flint-procgen/examples/living_spike.rs) poses a tree by
turning each rest segment with smooth 3D noise instead of regrowing it, through the
engine's own PoseStream, variance and blending. It writes a GLB per frame and four
held poses; this renders each with `flint edit <glb> --render` from one fixed camera
(the previewer would otherwise frame every pose to its own bounds, and the camera
would move with the tree), then writes to renders/wooze_props/noise/:

  <variant>.gif          4 s at 12 fps, the tree drifting on the linklater clock
  <variant>_poses.png    the first four poses held, side by side
  <a>__<b>.gif           two variants side by side, for the comparisons below

and prints, per variant, how many frame pairs change and by how much.
"""

import argparse
import re
import subprocess
import sys
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "renders" / "wooze_props" / "noise"
EXE = ".exe" if sys.platform == "win32" else ""
FLINT = ROOT / "target" / "release" / f"flint{EXE}"
SPIKE = ROOT / "target" / "release" / "examples" / f"living_spike{EXE}"

PINE = "specs/examples/pine_tree.procgen.toml"
OAK = "examples/specs/grove_oak.procgen.toml"
WILLOW = "specs/weeping_willow.procgen.toml"

# name -> (spec, lineage, living_spike flags, what it is)
VARIANTS = {
    "pine_noise": (PINE, 7, [], "space colonization, noise at the defaults: 12 deg, 3 m swells, per-level stagger"),
    "pine_still": (PINE, 7, ["--stagger", "none"], "the same with no stagger: the whole tree on one beat"),
    "pine_bend": (PINE, 7, ["--stagger", "none", "--turn", "20", "--wavelength", "1.5"],
                  "tighter swells, more turn: limbs bend along their length"),
    "oak_regrow": (OAK, 11, ["--source", "regrow"], "the grove's oak as it lives now: poses regrown"),
    "oak_noise": (OAK, 11, [], "the same oak, the same clock and variance, posed by noise"),
    "oak_noise_strong": (OAK, 11, ["--turn", "30", "--amount", "0.8"],
                         "noise pushed until it moves the oak about as far as regrowth does"),
    "pine_strong": (PINE, 7, ["--stagger", "none", "--turn", "30", "--wavelength", "2",
                              "--amount", "1.0", "--steady", "0.5"],
                    "the pine with all of its drift and big swells: where does it turn to jelly?"),
    "willow_noise": (WILLOW, 7, [], "a text-grammar willow, which can't regrow a pose at all"),
}
PAIRS = [("oak_regrow", "oak_noise"), ("oak_regrow", "oak_noise_strong"), ("pine_noise", "pine_still")]

SIZE = 480
FRAMES, FPS = 48, 12


def run(command: list) -> str:
    done = subprocess.run([str(c) for c in command], cwd=ROOT, capture_output=True, text=True)
    if done.returncode != 0:
        raise SystemExit(f"{Path(command[0]).name} failed:\n{done.stderr[-2000:]}")
    return done.stdout + done.stderr


def framing(glb: Path) -> list:
    """A camera that holds the whole tree, from its first frame's bounds."""
    text = run([FLINT, "edit", glb, "--render", OUT / "_probe.png", "--no-grid",
                "--width", "64", "--height", "64"])
    found = re.search(r"min \[([^\]]+)\], max \[([^\]]+)\]", text)
    if not found:
        raise SystemExit(f"no bounds in the previewer's output:\n{text}")
    lo = [float(v) for v in found.group(1).split(",")]
    hi = [float(v) for v in found.group(2).split(",")]
    centre = [(a + b) / 2 for a, b in zip(lo, hi)]
    extent = max(b - a for a, b in zip(lo, hi))
    target = ",".join(f"{v:.2f}" for v in centre)
    return [f"--target={target}", "--distance", f"{extent * 1.45:.2f}", "--yaw", "30", "--pitch", "8"]


def render(glb: Path, png: Path, camera: list) -> Image.Image:
    run([FLINT, "edit", glb, "--render", png, "--no-grid", "--width", SIZE, "--height", SIZE, *camera])
    return Image.open(png).convert("RGB")


def label(image: Image.Image, text: str) -> Image.Image:
    image = image.copy()
    ImageDraw.Draw(image).text((8, 6), text, fill=(235, 228, 210))
    return image


def save_gif(frames: list, path: Path) -> None:
    frames[0].save(path, save_all=True, append_images=frames[1:], duration=round(1000 / FPS), loop=0)


def motion(frames: list):
    arrays = [np.asarray(f, dtype=np.float32) for f in frames]
    diffs = [float(np.abs(b - a).mean()) for a, b in zip(arrays, arrays[1:])]
    moving = [d for d in diffs if d > 0.05]
    pattern = "".join("#" if d > 0.05 else "." for d in diffs)
    return len(moving), len(diffs), (sum(moving) / len(moving) if moving else 0.0), pattern


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--only", help="variants whose name starts with this")
    args = parser.parse_args()
    for tool, how in ((FLINT, "--bin flint"), (SPIKE, "-p flint-procgen --example living_spike")):
        if not tool.exists():
            raise SystemExit(f"build it first: cargo build --release {how}")
    OUT.mkdir(parents=True, exist_ok=True)

    gifs = {}
    print("Frame pairs at 12 fps that change (# moves, . holds), and what the prototype printed:\n")
    for name, (spec, lineage, flags, what) in VARIANTS.items():
        if args.only and not name.startswith(args.only):
            continue
        work = OUT / "frames" / name
        work.mkdir(parents=True, exist_ok=True)
        said = run([SPIKE, "noise", spec, "--out", work, "--lineage", lineage, "--frames", FRAMES,
                    "--fps", FPS, "--poses", "4", *flags])
        camera = framing(work / "f000.glb")
        frames = [label(render(work / f"f{f:03}.glb", work / f"f{f:03}.png", camera), name)
                  for f in range(FRAMES)]
        save_gif(frames, OUT / f"{name}.gif")
        gifs[name] = frames
        poses = [render(work / f"pose{k}.glb", work / f"pose{k}.png", camera) for k in range(4)]
        strip = Image.new("RGB", (SIZE * 4, SIZE))
        for k, pose in enumerate(poses):
            strip.paste(label(pose, f"{name} pose {k}"), (SIZE * k, 0))
        strip.save(OUT / f"{name}_poses.png")
        changed, pairs, mean, pattern = motion(frames)
        print(f"{name:13} {changed:2}/{pairs} move, mean {mean:5.2f}  {pattern}")
        print(f"              {what}")
        for line in said.strip().splitlines():
            print(f"              > {line}")
        print()

    for a, b in PAIRS:
        if a in gifs and b in gifs:
            both = []
            for fa, fb in zip(gifs[a], gifs[b]):
                frame = Image.new("RGB", (SIZE * 2, SIZE))
                frame.paste(fa, (0, 0))
                frame.paste(fb, (SIZE, 0))
                both.append(frame)
            save_gif(both, OUT / f"{a}__{b}.gif")
    (OUT / "_probe.png").unlink(missing_ok=True)
    print(f"GIFs and pose strips in {OUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
