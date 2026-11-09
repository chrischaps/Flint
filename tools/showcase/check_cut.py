"""Check an encoded cut frame by frame against what the edit composed (#436).

    python tools/showcase/check_cut.py                  # the master and the web MP4/WebM
    python tools/showcase/check_cut.py --only master

For each file it reads the stream (codec, size, pixel format, colour tags)
and every packet's timestamp, without decoding: is every frame there, once,
exactly a frame apart? Then it decodes every frame to a 160x90 luma thumbnail
and holds the film against the thumbnails edit.py saved of the frames it
composed. Each decoded frame is compared after taking out its mean offset
from the composed one (the YUV round trip shifts luma a level or two), so
what is left is the picture itself:

- **in place**: over every 2 s window, the decoded frames must match the
  composed ones best at a shift of 0, not -3..3 frames. A dropped or added
  frame shifts everything after it, and the window there matches best
  shifted.
- **not repeated**: where the composed film moves between two frames, the
  decoded film must move too. A duplicated frame is a step where it doesn't.
- **the right picture**: no frame is far from its own composed frame: more
  than 3x the median error of the second around it, plus a level. (The
  codec's error depends on the picture, ASCII's glyphs costing the most.)
- **not torn**: each of four horizontal bands of a frame must match the same
  band of its own composed frame. A frame whose top is one frame and bottom
  the next fails in one band and passes in another.

Steps where the composed film barely moves (the black of the fades) can't
show a repeat and are counted as `still`, not as failures. A web file at a
lower frame rate is checked against every k-th composed frame.

Writes `<name>.check.json` beside the cut. Exit 1 if any check fails.
"""

import argparse
import json
import os
import subprocess
import sys
import tomllib
from fractions import Fraction
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
HERE = ROOT / "tools" / "showcase"
TW, TH = 160, 90
BANDS = 4
WINDOW_SECONDS = 2.0
SHIFTS = range(-3, 4)
LUMA = np.array([0.2126, 0.7152, 0.0722], np.float32)


def ffprobe(path, *args):
    out = subprocess.run(["ffprobe", "-v", "error", *args, "-of", "json", str(path)],
                         capture_output=True, text=True, check=True).stdout
    return json.loads(out)


def stream_info(path):
    s = ffprobe(path, "-select_streams", "v:0", "-show_streams")["streams"][0]
    keep = ["codec_name", "profile", "level", "width", "height", "pix_fmt", "color_space",
            "color_transfer", "color_primaries", "color_range", "r_frame_rate",
            "avg_frame_rate", "bit_rate", "nb_frames"]
    return {k: s.get(k) for k in keep}


def packet_times(path):
    pk = ffprobe(path, "-select_streams", "v:0", "-show_entries", "packet=pts_time")
    return sorted(float(p["pts_time"]) for p in pk["packets"] if p.get("pts_time") is not None)


def decode_thumbs(path):
    """Every frame as 160x90 BT.709 luma, decoded to RGB by its own tags and
    area-averaged: the same steps edit.py took on the composed frames."""
    cmd = ["ffmpeg", "-v", "error", "-i", str(path), "-vf",
           f"scale={TW}:{TH}:flags=area,format=rgb24",
           "-fps_mode", "passthrough", "-f", "rawvideo", "-"]
    raw = subprocess.run(cmd, capture_output=True, check=True).stdout
    rgb = np.frombuffer(raw, np.uint8).reshape(-1, TH, TW, 3).astype(np.float32)
    return rgb @ LUMA


def centred(x):
    """Each frame minus its own mean: a level shift is not a different picture."""
    return x - x.mean(axis=(1, 2), keepdims=True)


def check(path, ref, step, fps):
    info = stream_info(path)
    times = packet_times(path)
    r = ref[::step].astype(np.float32)
    period = step / fps
    deltas = np.diff(times) if len(times) > 1 else np.array([])
    # WebM keeps milliseconds, so a 30 fps frame lasts 33 or 34 ms there.
    slack = max(period * 0.02, 0.0011)
    timing_ok = len(times) == len(r) and (len(deltas) == 0 or
                                          bool(np.all(np.abs(deltas - period) < slack)))
    timing = {"packets": len(times), "expected": len(r),
              "first_pts": times[0] if times else None,
              "min_step": float(deltas.min()) if len(deltas) else None,
              "max_step": float(deltas.max()) if len(deltas) else None,
              "period": period, "ok": timing_ok}

    got = decode_thumbs(path)
    n = min(len(got), len(r))
    g, rr = centred(got[:n]), centred(r[:n])
    err = np.abs(g - rr).mean(axis=(1, 2))  # per frame, after the level shift

    # In place: each window matches best unshifted.
    win = max(8, int(round(WINDOW_SECONDS * fps / step)))
    shifted = []
    for start in range(0, n, win):
        stop = min(n, start + win)
        scores = {}
        for k in SHIFTS:
            # decoded frame i against composed frame i + k, both inside the film
            lo, hi = max(start, -k), min(stop, n - k)
            if hi - lo < win // 2:
                continue
            scores[k] = float(np.abs(g[lo:hi] - rr[lo + k:hi + k]).mean())
        best = min(scores, key=scores.get)
        if best != 0 and scores[best] < scores[0] * 0.98:
            shifted.append({"frames": [start, stop - 1], "best_shift": best,
                            "error_at_0": round(scores[0], 3), "error_at_best": round(scores[best], 3)})

    # Not repeated: where the composed film moves, the decoded film moves.
    move_ref = np.abs(rr[1:] - rr[:-1]).mean(axis=(1, 2))
    move_got = np.abs(g[1:] - g[:-1]).mean(axis=(1, 2))
    floor = float(np.percentile(err, 50))  # the codec's own noise, per frame
    moving = move_ref > max(0.3, floor)
    repeats = [{"frame": int(i + 1), "ref_motion": round(float(move_ref[i]), 3),
                "decoded_motion": round(float(move_got[i]), 3)}
               for i in np.nonzero(moving & (move_got < move_ref * 0.25))[0]]

    # The codec's error depends on the picture (ASCII's glyphs cost more than
    # the plain render), so a frame is judged against the frames around it:
    # the median over the surrounding second.
    half = max(4, int(round(fps / step / 2)))

    def local_median(x):
        return np.array([np.median(x[max(0, i - half):i + half + 1]) for i in range(len(x))])

    # The right picture: no frame far from its own composed frame. A wrong or
    # broken frame is several times its neighbours' error (the first master:
    # median 0.45, max 0.77; the planted faults 7 to 33).
    limit = 3 * local_median(err) + 1.0
    wrong = [{"frame": int(i), "error": round(float(err[i]), 2), "limit": round(float(limit[i]), 2)}
             for i in np.nonzero(err > limit)[0]]

    # Not torn: every band of a frame matches its own frame about as well as
    # the others do (the first master's bands differed by at most 0.66).
    bands = np.stack([np.abs(g[:, b * TH // BANDS:(b + 1) * TH // BANDS]
                             - rr[:, b * TH // BANDS:(b + 1) * TH // BANDS]).mean(axis=(1, 2))
                      for b in range(BANDS)], axis=1)
    spread = bands.max(axis=1) - bands.min(axis=1)
    tear_limit = 3 * local_median(spread) + 1.0
    torn = [{"frame": int(i), "bands": [round(float(b), 2) for b in bands[i]]}
            for i in np.nonzero(spread > tear_limit)[0]]

    ok = bool(timing_ok and len(got) == len(r) and not shifted and not repeats and not torn
              and not wrong)
    return {
        "file": path.name,
        "bytes": path.stat().st_size,
        "stream": info,
        "timing": timing,
        "decoded_frames": int(len(got)),
        "composed_frames": int(len(r)),
        "error_mean": round(float(err.mean()), 3),
        "error_p99": round(float(np.percentile(err, 99)), 3),
        "error_max": round(float(err.max()), 3),
        "windows": int((n + win - 1) // win),
        "window_frames": win,
        "shifted_windows": shifted[:200],
        "moving_steps": int(moving.sum()),
        "still_steps": int((~moving).sum()),
        "repeats": repeats[:200],
        "repeat_count": len(repeats),
        "wrong": wrong[:200],
        "wrong_count": len(wrong),
        "torn": torn[:200],
        "torn_count": len(torn),
        "ok": ok,
    }


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0],
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--cut", default=str(HERE / "cut_16x9.toml"))
    ap.add_argument("--dir", help="the cut's folder (default: <takes>/cuts/<name>)")
    ap.add_argument("--only", choices=["master", "mp4", "webm"], action="append")
    args = ap.parse_args()

    cut = tomllib.loads(Path(args.cut).read_text(encoding="utf-8"))
    film = tomllib.loads((HERE / "film.toml").read_text(encoding="utf-8"))["film"]
    name = cut["cut"]["name"]
    folder = Path(args.dir) if args.dir else Path(os.path.expanduser(film["out"])) / "cuts" / name
    ref = np.load(folder / f"{name}.thumbs.npy")
    fps = cut["cut"]["fps"]
    web_fps = cut["web"].get("fps", fps)
    step = int(Fraction(fps, web_fps))

    files = {"master": (cut["master"]["file"], 1), "mp4": (cut["web"]["mp4"], step),
             "webm": (cut["web"]["webm"], step)}
    report_path = folder / f"{name}.check.json"
    results = json.loads(report_path.read_text()) if report_path.exists() else {}
    if isinstance(results, list):
        results = {}
    for key, (fname, k) in files.items():
        if args.only and key not in args.only:
            continue
        path = folder / fname
        if not path.exists():
            print(f"{key}: no {path}")
            continue
        r = check(path, ref, k, fps)
        results[key] = r
        s, t = r["stream"], r["timing"]
        print(f"{key}: {fname} {r['bytes'] / 1e6:.2f} MB, {s['codec_name']} {s['profile']} "
              f"{s['width']}x{s['height']} {s['pix_fmt']} {s['color_space']}/{s['color_primaries']}/"
              f"{s['color_transfer']} {s['color_range']} {s['r_frame_rate']}")
        print(f"  {t['packets']} packets ({t['expected']} expected), steps "
              f"{t['min_step']:.6f}..{t['max_step']:.6f} s; {r['decoded_frames']} frames decoded")
        print(f"  error mean {r['error_mean']} p99 {r['error_p99']} max {r['error_max']} (8-bit luma); "
              f"{len(r['shifted_windows'])} of {r['windows']} windows shifted; "
              f"{r['repeat_count']} repeats in {r['moving_steps']} moving steps "
              f"({r['still_steps']} still); {r['wrong_count']} wrong; "
              f"{r['torn_count']} torn  -> {'ok' if r['ok'] else 'FAILED'}")
    report_path.write_text(json.dumps(results, indent=2), encoding="utf-8")
    sys.exit(0 if results and all(r["ok"] for r in results.values()) else 1)


if __name__ == "__main__":
    main()
