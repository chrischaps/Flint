"""What else gets the wooze, judged by eye (#252): every variant tried, rendered the same way.

    cargo build --release --bin flint --bin flint-player
    cargo build --release -p flint-procgen --example living_spike
    python demo/wooze_props/experiment.py               # all of it, about 20 minutes
    python demo/wooze_props/experiment.py --only beetle # the variants whose name starts so

For each variant of demo/wooze_props.scene.toml it writes a 4-second GIF from the
scene's camera and one low over the beetles to renders/wooze_props/, and prints how
each target moves: of the frame pairs at 12 fps, how many change in a crop around
it, and by how much. Each target stands beside a still twin, whose crop is the
control. Then the bob, in flint-player, since scripts don't run under flint render,
and then noise_pose.py, the fourth target, which isn't in the engine (--no-noise
skips it).

A variant is the scene (and, for the vine, its spec) with a few lines changed,
written beside the originals as dotfiles so every path resolves as the scene's own,
and removed afterwards.
"""

import argparse
import re
import subprocess
import sys
from pathlib import Path

import numpy as np
from PIL import Image

ROOT = Path(__file__).resolve().parents[2]
SCENE = ROOT / "demo" / "wooze_props.scene.toml"
VINE = ROOT / "demo" / "specs" / "grove_vine.procgen.toml"
OUT = ROOT / "renders" / "wooze_props"
EXE = ".exe" if sys.platform == "win32" else ""
FLINT = ROOT / "target" / "release" / f"flint{EXE}"
PLAYER = ROOT / "target" / "release" / f"flint-player{EXE}"

# name -> (scene lines, vine [living] lines, what it is). A scene line is matched by
# its key inside the named entity's table: "beetle_left.shallow_wooze.amplitude".
VARIANTS = {
    "authored": ({}, {}, "as the scene and spec are: vine at variance 0.35/0.8, beetle 0.04 m in 0.35 m swells"),
    "vine_full": ({}, {"variance": "{ amount = 1.0, steady = 0.5, keep_size = true }"},
                  "the vine with all of its drift, and less held near the stem"),
    "vine_bare": ({}, {"echoes": "{ count = 0 }", "jitter": "{ amplitude = 0.0, targets = [\"leaf_cards\"] }"},
                  "the vine with no echoes and no leaf jitter: drift alone"),
    "beetle_deep": ({"beetle_left.shallow_wooze.amplitude": "0.08",
                     "beetle_left.shallow_wooze.wavelength": "0.25"}, {},
                    "the beetle twice as far in tighter swells"),
    "beetle_whole": ({"beetle_left.shallow_wooze.wavelength": "3.0"}, {},
                     "the beetle in swells far bigger than it: it moves whole instead of changing shape"),
}

# The measuring pass: smaller, and with the shading boil off, since the boil redraws
# every pixel 12 times a second and would move the still twins' crops too.
MEASURE = ["--kuwahara-boil", "0", "--contour-boil", "0"]

WIDTH, HEIGHT = 960, 540
SPAN, FPS = "0..4", "12"
# name -> (flint render camera flags, crops as fractions of the frame (x0, y0, x1, y1),
# each holding one target and the sky or ground behind it).
SHOTS = {
    # The scene's camera. The oak's crown stays out of both vine crops.
    "wide": ([], {
        "vine (living)": (0.25, 0.10, 0.385, 0.42),
        "vine (baked)": (0.59, 0.10, 0.73, 0.42),
        "beetle (wooze)": (0.34, 0.74, 0.43, 0.90),
        "beetle (still)": (0.57, 0.74, 0.67, 0.90),
    }),
    # Low over the beetles from the far side, in profile: the woozy one is on the right.
    "beetles": (["--target=0,0.3,-1.6", "--distance", "2.8", "--pitch", "18", "--yaw", "0"], {
        "beetle (wooze)": (0.57, 0.42, 0.78, 0.70),
        "beetle (still)": (0.22, 0.42, 0.43, 0.70),
    }),
}


def set_scene_line(text: str, key: str, value: str) -> str:
    entity, table, field = key.split(".")
    header = f"[entities.{entity}.{table}]"
    start = text.index(header)
    end = text.find("\n[", start + len(header))
    end = len(text) if end < 0 else end
    block = text[start:end]
    block, n = re.subn(rf"^{field} = [^\n]*", f"{field} = {value}", block, flags=re.M)
    if n != 1:
        raise SystemExit(f"{key}: found {n} times")
    return text[:start] + block + text[end:]


def variant_files(name: str, scene_lines: dict, vine_lines: dict) -> list:
    """Write the variant's scene (and vine spec), returning every file to remove."""
    text = SCENE.read_text(encoding="utf-8")
    for key, value in scene_lines.items():
        text = set_scene_line(text, key, value)
    written = []
    if vine_lines:
        # A spec of its own, so the baked twin and the committed spec stay as they are.
        spec_name = f"grove_vine_{name}"
        spec = VINE.read_text(encoding="utf-8").replace('name = "grove_vine"', f'name = "{spec_name}"')
        for key, value in vine_lines.items():
            spec, n = re.subn(rf"^{key} = [^\n]*", f"{key} = {value}", spec, flags=re.M)
            if n != 1:
                raise SystemExit(f"vine {key}: found {n} times")
        spec_path = VINE.with_name(f"{spec_name}.procgen.toml")
        spec_path.write_text(spec, encoding="utf-8")
        written.append(spec_path)
        text = set_scene_line(text, "vine_left.model.asset", f'"{spec_name}"')
    scene = SCENE.with_name(f".wooze_props_{name}.scene.toml")
    scene.write_text(text, encoding="utf-8")
    return [scene, *written]


def render(scene: Path, out: Path, camera: list, measure: bool = False) -> None:
    extra = MEASURE if measure else []
    command = [
        str(FLINT), "render", str(scene), "--schemas", str(ROOT / "schemas"),
        "--time", SPAN, "--fps", FPS, "--width", str(WIDTH), "--height", str(HEIGHT),
        "--output", str(out), *camera, *extra,
    ]
    done = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
    if done.returncode != 0:
        raise SystemExit(f"flint render failed for {out.name}:\n{done.stderr[-2000:]}")


def motion(gif: Path, crops: dict) -> dict:
    """Per crop: frame pairs that change, their mean change (0-255), and the pattern."""
    image = Image.open(gif)
    frames = []
    for i in range(image.n_frames):
        image.seek(i)
        frames.append(np.asarray(image.convert("RGB"), dtype=np.float32))
    h, w, _ = frames[0].shape
    found = {}
    for crop, (x0, y0, x1, y1) in crops.items():
        cut = [f[int(y0 * h):int(y1 * h), int(x0 * w):int(x1 * w)] for f in frames]
        diffs = [float(np.abs(b - a).mean()) for a, b in zip(cut, cut[1:])]
        moving = [d for d in diffs if d > 0.05]
        found[crop] = (len(moving), len(diffs), sum(moving) / len(moving) if moving else 0.0,
                       "".join("#" if d > 0.05 else "." for d in diffs))
    return found


# The bob (#251's question): the showcase creatures' idle bobs and sways on the wall
# clock. Scripts don't run under flint render, so this one plays in flint-player, one
# frame per 1/12 s of game time, from a camera low over the beetles (no player, so
# the scene's [camera] is the view). Both beetles run demo/scripts/beetle_idle.rhai;
# the left one (on screen too) also has shallow wooze. A run without the scripts is
# the control.
BOB_CAMERA = {"position": "[0.0, 0.95, -2.0]", "target": "[0.0, 0.3, 0.4]"}
BOB_CROPS = {
    "beetle (wooze + bob)": (0.28, 0.44, 0.44, 0.68),
    "beetle (bob only)": (0.57, 0.44, 0.72, 0.68),
}


def bob_scene(name: str, scripts: bool, boil: bool) -> Path:
    text = SCENE.read_text(encoding="utf-8")
    text = text[:text.index("[entities.player]")] + text[text.index("# --- Behind"):]
    camera = text.index("[camera]")
    for key, value in BOB_CAMERA.items():
        text = text[:camera] + re.sub(rf"^{key} = [^\n]*", f"{key} = {value}", text[camera:], count=1, flags=re.M)
    for side, x in (("left", "0.6"), ("right", "-0.6")):
        text = set_scene_line(text, f"beetle_{side}.transform.position", f"[{x}, 0.0, 0.4]")
        if scripts:
            model = f"[entities.beetle_{side}.model]"
            script = f'[entities.beetle_{side}.script]\nsource = "beetle_idle.rhai"\n\n'
            text = text.replace(model, script + model)
    if not boil:
        for key in ("kuwahara_boil", "contour_boil"):
            text, n = re.subn(rf"^{key} = [^\n]*", f"{key} = 0.0", text, flags=re.M)
            if n != 1:
                raise SystemExit(f"{key}: found {n} times")
    path = SCENE.with_name(f".wooze_props_{name}.scene.toml")
    path.write_text(text, encoding="utf-8")
    return path


def play(scene: Path, gif: Path) -> None:
    """4 s of the player at 12 frames a second of game time, as a GIF."""
    frames_dir = OUT / "measure" / f"_{gif.stem}"
    frames_dir.mkdir(parents=True, exist_ok=True)
    for old in frames_dir.glob("*.png"):
        old.unlink()
    step = f"{1 / 12:.7f}"
    command = [
        str(PLAYER), str(scene), "--schemas", str(ROOT / "schemas"), "--width", str(WIDTH),
        "--height", str(HEIGHT), "--frame-step", step, "--screenshot", str(frames_dir / "f.png"),
        "--screenshot-every", step, "--exit-after", "4",
    ]
    done = subprocess.run(command, cwd=ROOT, capture_output=True, text=True, timeout=300)
    if done.returncode != 0:
        raise SystemExit(f"flint-player failed for {gif.name}:\n{done.stderr[-2000:]}")
    frames = [Image.open(f).convert("RGB") for f in sorted(frames_dir.glob("f_*.png"))[:49]]
    frames[0].save(gif, save_all=True, append_images=frames[1:], duration=83, loop=0)


def bob(watch: bool) -> None:
    if not PLAYER.exists():
        raise SystemExit(f"build it first: cargo build --release -p flint-player ({PLAYER})")
    print("bob: both beetles bob and sway on the wall clock; the left one also has shallow wooze")
    runs = [("bob", True, True, watch), ("bob_measure", True, False, False),
            ("bob_control", False, False, False)]
    for name, scripts, boil, keep in runs:
        if not keep and boil:
            continue
        scene = bob_scene(name, scripts, boil)
        try:
            gif = OUT / f"{name}.gif" if boil else OUT / "measure" / f"{name}.gif"
            play(scene, gif)
        finally:
            scene.unlink(missing_ok=True)
        if boil:
            continue
        label = "scripts" if scripts else "control, no scripts"
        for crop, (changed, pairs, mean, pattern) in motion(gif, BOB_CROPS).items():
            print(f"  {label:19} {crop:20} {changed:2}/{pairs} move, mean {mean:5.2f}  {pattern}")
    print()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--only", help="variants whose name starts with this")
    parser.add_argument("--measure-only", action="store_true", help="skip the GIFs to watch")
    parser.add_argument("--no-bob", action="store_true", help="skip the flint-player run")
    parser.add_argument("--no-noise", action="store_true", help="skip noise_pose.py")
    args = parser.parse_args()
    if not FLINT.exists():
        raise SystemExit(f"build it first: cargo build --release --bin flint ({FLINT})")
    (OUT / "measure").mkdir(parents=True, exist_ok=True)

    print("Frame pairs at 12 fps whose crop changes (# moves, . holds), boil off:\n")
    for name, (scene_lines, vine_lines, what) in VARIANTS.items():
        if args.only and not name.startswith(args.only):
            continue
        files = variant_files(name, scene_lines, vine_lines)
        try:
            print(f"{name}: {what}")
            for shot, (camera, crops) in SHOTS.items():
                if not args.measure_only:
                    render(files[0], OUT / f"{name}_{shot}.gif", camera)
                gif = OUT / "measure" / f"{name}_{shot}.gif"
                render(files[0], gif, camera, measure=True)
                for crop, (changed, pairs, mean, pattern) in motion(gif, crops).items():
                    print(f"  {shot:7} {crop:15} {changed:2}/{pairs} move, mean {mean:5.2f}  {pattern}")
            print()
        finally:
            for path in files:
                path.unlink(missing_ok=True)
    if not args.no_bob and (not args.only or "bob".startswith(args.only) or args.only == "bob"):
        bob(watch=not args.measure_only)
    print(f"GIFs to watch in {OUT.relative_to(ROOT)}, measured ones in measure/\n")
    if not args.no_noise and not args.only:
        noise = [sys.executable, str(Path(__file__).with_name("noise_pose.py"))]
        subprocess.run(noise, cwd=ROOT, check=True)


if __name__ == "__main__":
    main()
