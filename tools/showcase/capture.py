"""Capture the showcase film's beats, unattended, as PNG frame sequences (#435).

    python tools/showcase/capture.py                    # every beat, every take
    python tools/showcase/capture.py 02_type 08_pencil  # some beats
    python tools/showcase/capture.py --scale 0.5        # 1920x1080 preview frames
    python tools/showcase/capture.py --verify 02_type   # capture twice and compare
    python tools/showcase/capture.py --list             # beats, takes, frames, disk

Each take is one run of `flint-player` on the beat's scene with the clock
stepped exactly 1/fps a frame (`--frame-step`) and every frame written
(`--screenshot-every`), so a slow 4K frame changes nothing on screen: the
run is the same however long it takes. Takes land in

    <out>/<beat>/<take>/frame_0001.png ...

with a `take.json` beside the frames saying how they were made. `--verify`
captures a take twice into `<take>` and `<take>.again` and compares them
frame by frame (`compare.py`); the report says which frames differ, in how
many pixels and by how much (see issue #341 for why they can).

Beats are listed in tools/showcase/film.toml; their scenes are written by
build_scenes.py, which this runs first. The player is the release build
(`cargo build --release --bin flint-player`), found in target/release.

Needs Python 3.11+; --verify also needs Pillow and numpy.
"""

import argparse
import json
import os
import shutil
import subprocess
import sys
import time
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HERE = ROOT / "tools" / "showcase"
POINTS_WIDE = 1920  # the canvas point-sized things are laid out on


def player_path():
    exe = "flint-player.exe" if os.name == "nt" else "flint-player"
    path = ROOT / "target" / "release" / exe
    if not path.exists():
        raise SystemExit(
            f"no {path.relative_to(ROOT)}: cargo build --release --bin flint-player"
        )
    return path


def takes_of(beat):
    return beat.get("take") or [{"name": "main"}]


def frame_count(film, beat):
    """Frames a take writes: one at t = 0, then one a step to `seconds`."""
    return int(round(beat["seconds"] * film["fps"])) + 1


def scaled_input(source, scale, dest):
    """Copy an input script with its pointer positions scaled from points
    to the take's pixels: `mouse x y`, `click x y`, `touch id x y` and
    `drag id x y` move; `look` is raw motion and keeps its units."""
    out = []
    for line in source.read_text(encoding="utf-8").splitlines():
        body, hash_, comment = line.partition("#")
        words = body.split()
        if len(words) >= 4 and words[1] in ("mouse", "click", "touch", "drag"):
            first = 2 if words[1] in ("mouse", "click") else 3
            for i in (first, first + 1):
                words[i] = f"{float(words[i]) * scale:.1f}"
            body = "  ".join(words) + "  "
        out.append(body + hash_ + comment)
    dest.write_text("\n".join(out) + "\n", encoding="utf-8")
    return dest


def take_args(film, beat, take, width, height, folder):
    """The flint-player command line for one take."""
    step = 1.0 / film["fps"]
    args = [
        str(player_path()),
        str(Path("demo") / "film" / f"{beat['id']}.scene.toml"),
        "--schemas", "schemas",
        "--width", str(width),
        "--height", str(height),
        "--pixels-per-point", f"{width / POINTS_WIDE:.6g}",
        "--frame-step", f"{step:.9f}",
        "--screenshot", str(folder / "frame.png"),
        "--screenshot-every", f"{step:.9f}",
        # Half a step short of `seconds`, so float sums of the step neither
        # drop the last frame nor add one past it.
        "--exit-after", f"{beat['seconds'] - step * 0.5:.9f}",
        "--music-volume", "0",
        "--sfx-volume", "0",
    ]
    script = take.get("input") or beat.get("input")
    if script:
        source = ROOT / "demo" / "film" / "beats" / script
        scaled = scaled_input(source, width / POINTS_WIDE, folder / "input.txt")
        args += ["--input-script", str(scaled)]
    args += take.get("args", []) + beat.get("args", [])
    return args


def capture(film, beat, take, width, height, folder):
    if folder.exists():
        shutil.rmtree(folder)
    folder.mkdir(parents=True)
    args = take_args(film, beat, take, width, height, folder)
    started = time.time()
    log = folder / "player.log"
    with open(log, "w", encoding="utf-8") as out:
        result = subprocess.run(args, cwd=ROOT, stdout=out, stderr=subprocess.STDOUT)
    seconds = time.time() - started
    frames = sorted(folder.glob("frame_*.png"))
    record = {
        "beat": beat["id"],
        "take": take["name"],
        "title": beat["title"],
        "features": beat.get("features", []),
        "width": width,
        "height": height,
        "fps": film["fps"],
        "frames": len(frames),
        "expected_frames": frame_count(film, beat),
        "wall_seconds": round(seconds, 1),
        "exit_code": result.returncode,
        "command": [Path(args[0]).name] + args[1:],
    }
    (folder / "take.json").write_text(json.dumps(record, indent=2), encoding="utf-8")
    status = "ok" if result.returncode == 0 and frames else f"FAILED (exit {result.returncode})"
    print(
        f"  {beat['id']}/{take['name']}: {len(frames)} frames "
        f"({record['expected_frames']} expected) in {seconds:.0f} s  {status}"
    )
    if result.returncode != 0 or not frames:
        print(f"    see {log}")
    return record


def free_bytes(path):
    probe = path
    while not probe.exists():
        probe = probe.parent
    return shutil.disk_usage(probe).free


def main():
    ap = argparse.ArgumentParser(
        description=__doc__.splitlines()[0],
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    ap.add_argument("beats", nargs="*", help="beat ids (default: all)")
    ap.add_argument("--take", action="append", help="only these takes")
    ap.add_argument("--out", help="output folder (default: film.toml's [film] out)")
    ap.add_argument("--scale", type=float, default=1.0,
                    help="fraction of the master resolution (0.5 = 1920x1080)")
    ap.add_argument("--width", type=int, help="frame width (overrides --scale)")
    ap.add_argument("--height", type=int, help="frame height (overrides --scale)")
    ap.add_argument("--verify", action="store_true",
                    help="capture each take twice and compare the runs")
    ap.add_argument("--list", action="store_true", help="list beats and exit")
    ap.add_argument("--no-build", action="store_true", help="don't rebuild the scenes first")
    args = ap.parse_args()

    film_toml = tomllib.loads((HERE / "film.toml").read_text(encoding="utf-8"))
    film = film_toml["film"]
    beats = film_toml["beat"]
    width = args.width or int(round(film["width"] * args.scale))
    height = args.height or int(round(film["height"] * args.scale))
    out = Path(os.path.expanduser(args.out or film["out"])).resolve()

    unknown = [b for b in args.beats if b not in {x["id"] for x in beats}]
    if unknown:
        raise SystemExit(f"unknown beat(s) {unknown}; --list shows them")
    chosen = [b for b in beats if not args.beats or b["id"] in args.beats]

    runs = []
    for beat in chosen:
        for take in takes_of(beat):
            if args.take and take["name"] not in args.take:
                continue
            runs.append((beat, take))

    # A 4K PNG of these scenes averaged 7.0 MB at 60 fps (#436; up to 18 MB
    # for the busiest); estimate 8 MB and refuse a run that can't fit
    per_frame = 8e6 * (width * height) / (3840 * 2160)
    frames = sum(frame_count(film, b) for b, _ in runs) * (2 if args.verify else 1)
    need = frames * per_frame

    if args.list:
        for beat in beats:
            takes = ", ".join(t["name"] for t in takes_of(beat))
            print(f"{beat['id']:<16} {beat['seconds']:>5.1f} s  {beat['title']}  [{takes}]")
        print(f"\n{len(runs)} takes, {frames} frames at {width}x{height}: ~{need / 1e9:.1f} GB")
        return

    if need > free_bytes(out) * 0.9:
        raise SystemExit(
            f"{frames} frames at {width}x{height} need ~{need / 1e9:.1f} GB and "
            f"{out} has {free_bytes(out) / 1e9:.1f} GB free: fewer beats, --scale, or --out"
        )
    if not args.no_build:
        subprocess.run([sys.executable, str(HERE / "build_scenes.py")], cwd=ROOT, check=True)

    print(f"{film['title']}: {len(runs)} takes at {width}x{height}, "
          f"{film['fps']} fps, into {out} (~{need / 1e9:.1f} GB)")
    failed = False
    reports = []
    for beat, take in runs:
        folder = out / beat["id"] / take["name"]
        record = capture(film, beat, take, width, height, folder)
        failed |= record["exit_code"] != 0 or record["frames"] == 0
        if args.verify:
            again = out / beat["id"] / f"{take['name']}.again"
            record2 = capture(film, beat, take, width, height, again)
            failed |= record2["exit_code"] != 0
            sys.path.insert(0, str(HERE))
            from compare import compare_folders

            report = compare_folders(folder, again)
            report.update(beat=beat["id"], take=take["name"])
            (folder / "verify.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
            reports.append(report)
            print("    " + report["summary"])
    if reports:
        (out / "verify.json").write_text(json.dumps(reports, indent=2), encoding="utf-8")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
