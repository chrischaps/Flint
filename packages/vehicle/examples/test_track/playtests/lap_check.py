#!/usr/bin/env python3
"""The vehicle package's lap, as CI plays it (#1629, epic #1626).

Plays the test track's lap (lap.input, judged by referee.rhai) through
`flint playtest` at 1/60 s frames, then at 1/30 and 1/144 s, and fails when
any run fails (the kart left the track, flipped or didn't finish) or when
the slowest and fastest laps are more than 0.1 s apart (ADR 0140 decision 8,
the epic's frame-rate gate). No GPU: the playtest draws nothing.

  python3 packages/vehicle/examples/test_track/playtests/lap_check.py --flint target/debug/flint

Standard library only.
"""

import argparse
import json
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCENE = HERE.parent / "test_track.scene.toml"
RATES = [("1/60", 1 / 60), ("1/30", 1 / 30), ("1/144", 1 / 144)]
TOLERANCE = 0.1  # seconds a lap, across the frame rates


def play(flint: str, step: float, report: Path) -> dict:
    cmd = [
        flint, "playtest", str(SCENE),
        "--input-script", str(HERE / "lap.input"),
        "--test-script", str(HERE / "referee.rhai"),
        "--timeout", "30",
        "--frame-step", repr(step),
        "--report", str(report),
    ]
    run = subprocess.run(cmd, capture_output=True, text=True)
    if not report.exists():
        sys.stderr.write(run.stdout + run.stderr)
        raise SystemExit(f"flint playtest wrote no report (exit {run.returncode})")
    return json.loads(report.read_text(encoding="utf-8"))


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--flint", default="flint", help="the flint binary")
    args = ap.parse_args()
    exe = Path(args.flint).with_suffix(".exe")
    if not Path(args.flint).exists() and exe.exists():
        args.flint = str(exe)  # target/debug/flint on Windows

    laps = {}
    failed = False
    with tempfile.TemporaryDirectory() as tmp:
        for name, step in RATES:
            r = play(args.flint, step, Path(tmp) / "report.json")
            log = r.get("log", {})
            if not r.get("passed"):
                failed = True
                for f in r.get("failures", []):
                    print(f"FAIL at {name} s frames: {f.get('kind')}: {f.get('message')}")
                continue
            laps[name] = log["lap_time"]
            print(
                f"PASS at {name} s frames: lap {log['lap_time']:.3f} s, top speed "
                f"{log['top_speed']:.1f} m/s, {log['longest_airtime']:.2f} s in the air, "
                f"at most {log['widest_off_centre']:.2f} m off the centre line"
            )
    if failed:
        return 1
    spread = max(laps.values()) - min(laps.values())
    if spread > TOLERANCE:
        print(f"FAIL: the laps are {spread:.3f} s apart across frame rates (allowed {TOLERANCE} s)")
        return 1
    print(f"the laps agree within {spread:.3f} s across frame rates (allowed {TOLERANCE} s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
