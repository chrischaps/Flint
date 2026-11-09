"""Check a cut's encoded files frame by frame, and their sound (#436, #593).

    python tools/showcase/check_cut.py master          # every file cut.py wrote for it
    python tools/showcase/check_cut.py reel            # the reel and its stories
    python tools/showcase/check_cut.py master --only master --only webm
    python tools/showcase/check_cut.py master --dir D:/film/master/edit

It reads the cut's `edit.json` (in `<film out>/<cut>/edit/`, or --dir), which
lists every file cut.py encoded and which of the composed frames each holds,
and the thumbnails cut.py saved of those frames.

**The picture.** For each file it reads the stream (codec, size, pixel
format, colour tags) and every packet's timestamp, without decoding: is
every frame there, once, exactly a frame apart? Then it decodes every frame
to a small luma thumbnail and holds the film against the composed ones. Each
decoded frame is compared after taking out its mean offset from the
composed one (the YUV round trip shifts luma a level or two), so what is
left is the picture itself:

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

Steps where the composed film barely moves, or between two frames with
under 2 levels of detail left (the black of the fades), can't show a repeat
and are counted as `still`, not as failures. A web file at a
lower frame rate is checked against every k-th composed frame.

**The sound** (#593). Each file's audio is decoded as a player would play
it (edit lists and Opus pre-skip applied) and measured with the meters the
music was mastered with (synth.py, BS.1770-4):

- **there**, unless the cut was made `--silent`: 48 kHz stereo;
- **as long as the picture**, to within one of the file's frames;
- **at the music's loudness**: a file of the whole cut within 0.5 LU of
  -14 LUFS integrated (a story piece is a slice, so its loudness isn't);
- **not clipped**: true peak at most -1.0 dBTP (the master's -1.5 plus a
  half dB for the lossy codec) and no sample at full scale.

**The bars.** The music's report (`many-hands-<cut>.json`, beside the WAV)
must still describe the WAV, be this cut's, and start every shot on the
frame the edit started it: every cut on the music's.

Writes `<cut>.check.json` beside the films. Exit 1 if any check fails.
"""

import argparse
import hashlib
import json
import os
import subprocess
import sys
import tomllib
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
HERE = ROOT / "tools" / "showcase"
sys.path.insert(0, str(HERE))
import synth  # noqa: E402

BANDS = 4
WINDOW_SECONDS = 2.0
SHIFTS = range(-3, 4)
LUMA = np.array([0.2126, 0.7152, 0.0722], np.float32)
TARGET_LUFS = -14.0
LUFS_SLACK = 0.5
PEAK_LIMIT_DBTP = -1.0
DARK_CONTRAST = 2.0  # levels of luma (std of a thumbnail) below which a step is still


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


def decode_thumbs(path, tw, th):
    """Every frame as tw x th BT.709 luma, decoded to RGB by its own tags and
    area-averaged: the same steps cut.py took on the composed frames."""
    cmd = ["ffmpeg", "-v", "error", "-i", str(path), "-map", "0:v:0", "-vf",
           f"scale={tw}:{th}:flags=area,format=rgb24",
           "-fps_mode", "passthrough", "-f", "rawvideo", "-"]
    raw = subprocess.run(cmd, capture_output=True, check=True).stdout
    rgb = np.frombuffer(raw, np.uint8).reshape(-1, th, tw, 3).astype(np.float32)
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

    got = decode_thumbs(path, ref.shape[2], ref.shape[1])
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
        if 0 not in scores:
            continue  # a last window too short to shift: the one before it covers it
        best = min(scores, key=scores.get)
        if best != 0 and scores[best] < scores[0] * 0.98:
            shifted.append({"frames": [start, stop - 1], "best_shift": best,
                            "error_at_0": round(scores[0], 3), "error_at_best": round(scores[best], 3)})

    # Not repeated: where the composed film moves, the decoded film moves.
    move_ref = np.abs(rr[1:] - rr[:-1]).mean(axis=(1, 2))
    move_got = np.abs(g[1:] - g[:-1]).mean(axis=(1, 2))
    floor = float(np.percentile(err, 50))  # the codec's own noise, per frame
    # Two frames with almost no detail left (the last of a fade to black)
    # can't show a repeat: 8-bit YUV rounds what is left of them to one
    # level. The film itself never drops below 14 levels (#593's master).
    contrast = rr.std(axis=(1, 2))
    legible = np.minimum(contrast[1:], contrast[:-1]) > DARK_CONTRAST
    moving = (move_ref > max(0.3, floor)) & legible
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
    th = ref.shape[1]
    bands = np.stack([np.abs(g[:, b * th // BANDS:(b + 1) * th // BANDS]
                             - rr[:, b * th // BANDS:(b + 1) * th // BANDS]).mean(axis=(1, 2))
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


def sound(path, seconds, frame_s, whole, silent):
    """The file's audio as played: there, as long as the picture, at the
    music's loudness, unclipped."""
    streams = ffprobe(path, "-select_streams", "a", "-show_streams")["streams"]
    if not streams:
        return {"ok": silent, "present": False,
                "problems": [] if silent else ["no audio stream"]}
    a = streams[0]
    raw = subprocess.run(["ffmpeg", "-v", "error", "-i", str(path), "-map", "0:a:0",
                          "-f", "f32le", "-ac", "2", "-ar", str(synth.SR), "-"],
                         capture_output=True, check=True).stdout
    x = np.frombuffer(raw, "<f4").reshape(-1, 2).astype(np.float64)
    length = len(x) / synth.SR
    r = {"present": True, "codec": a.get("codec_name"), "sample_rate": int(a["sample_rate"]),
         "channels": a.get("channels"), "bit_rate": a.get("bit_rate"),
         "seconds": round(length, 6), "video_seconds": round(seconds, 6),
         "off_by_s": round(length - seconds, 6)}
    problems = []
    if r["sample_rate"] != synth.SR or r["channels"] != 2:
        problems.append(f"{r['sample_rate']} Hz, {r['channels']} channels")
    if abs(length - seconds) > frame_s + 1e-9:
        problems.append(f"audio {length:.4f} s, picture {seconds:.4f} s")
    if not silent:
        lufs = synth.integrated_lufs(x)
        r.update(integrated_lufs=round(lufs, 2), true_peak_dbtp=round(synth.true_peak_db(x), 2),
                 full_scale_samples=int(np.sum(np.abs(x) >= 1.0)))
        if whole and abs(lufs - TARGET_LUFS) > LUFS_SLACK:
            problems.append(f"{lufs:.2f} LUFS, not {TARGET_LUFS:g}")
        if r["true_peak_dbtp"] > PEAK_LIMIT_DBTP:
            problems.append(f"true peak {r['true_peak_dbtp']} dBTP")
        if r["full_scale_samples"]:
            problems.append(f"{r['full_scale_samples']} samples at full scale")
    r["problems"] = problems
    r["ok"] = not problems
    return r


def bars(edit):
    """Every shot starts on the music's frame, and the music's report still
    describes its WAV."""
    music = edit.get("music")
    if not music:
        silent = edit.get("audio") == "silence"
        return {"ok": silent, "problems": [] if silent else ["no music report"]}
    report = Path(music["report"])
    if not report.exists():
        return {"ok": False, "problems": [f"no {report}"]}
    m = json.loads(report.read_text(encoding="utf-8"))
    wav = Path(edit["audio"])
    problems = []
    if m.get("cut") != edit["cut"]:
        problems.append(f"the music is the {m.get('cut')!r} cut's")
    if m["sha256"] != music["sha256"]:
        problems.append("the music was rebuilt after this edit: cut it again")
    elif wav.exists() and hashlib.sha256(wav.read_bytes()).hexdigest() != m["sha256"]:
        problems.append(f"{wav.name} isn't the file its report describes")
    if abs(m["samples"] / m["sample_rate"] - edit["frames"] / edit["fps"]) > 0.5 / m["sample_rate"]:
        problems.append(f"the music is {m['seconds']} s, the picture {edit['seconds']} s")
    scored = [s for s in m["shots"] if s["beat"] != "tail"]
    if len(scored) != len(edit["shots"]):
        problems.append(f"{len(scored)} shots in the music, {len(edit['shots'])} in the edit")
    on = 0
    for s, ms in zip(edit["shots"], scored):
        at = int(round(ms["at_s"] * edit["fps"]))
        if (s["beat"], s["take"]) == (ms["beat"], ms["take"]) and s["first"] == at:
            on += 1
        else:
            problems.append(f"{s['beat']}/{s['take']} starts on frame {s['first']}, "
                            f"the music's {ms['beat']}/{ms['take']} on {at}")
    return {"ok": not problems, "shots": len(edit["shots"]), "on_their_frames": on,
            "problems": problems, "lufs": m.get("integrated_lufs"),
            "true_peak_dbtp": m.get("true_peak_dbtp")}


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0],
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("cut", nargs="?", default="master", help="a cut in tools/showcase/cuts/")
    ap.add_argument("--dir", help="the cut's edit folder (default <film out>/<cut>/edit)")
    ap.add_argument("--only", action="append", help="only these files, by key (master, mp4, "
                    "webm, story_1 ...)")
    args = ap.parse_args()

    film = tomllib.loads((HERE / "film.toml").read_text(encoding="utf-8"))["film"]
    folder = (Path(args.dir) if args.dir
              else Path(os.path.expanduser(film["out"])) / args.cut / "edit")
    record = folder / "edit.json"
    if not record.exists():
        raise SystemExit(f"no {record}: python tools/showcase/cut.py {args.cut}")
    edit = json.loads(record.read_text(encoding="utf-8"))
    ref = np.load(folder / edit["thumbs"])
    fps = edit["fps"]
    silent = edit.get("audio") == "silence"

    results = {"bars": bars(edit)}
    b = results["bars"]
    print(f"bars: {b.get('on_their_frames', 0)} of {b.get('shots', 0)} shots start on the "
          f"music's frames  -> {'ok' if b['ok'] else 'FAILED'}")
    for p in b["problems"]:
        print(f"  {p}")
    for f in edit["files"]:
        if args.only and f["key"] not in args.only:
            continue
        path = folder / f["file"]
        if not path.exists():
            results[f["key"]] = {"file": f["file"], "ok": False, "problems": ["missing"]}
            print(f"{f['key']}: no {path}")
            continue
        step = f["step"]
        part = ref[f["first"]:f["first"] + f["frames"] * step]
        r = check(path, part, step, fps)
        r["sound"] = sound(path, f["frames"] * step / fps, step / fps, f["whole"], silent)
        r["ok"] = r["ok"] and r["sound"]["ok"]
        results[f["key"]] = r
        s, t, a = r["stream"], r["timing"], r["sound"]
        print(f"{f['key']}: {f['file']} {r['bytes'] / 1e6:.2f} MB, {s['codec_name']} {s['profile']} "
              f"{s['width']}x{s['height']} {s['pix_fmt']} {s['color_space']}/{s['color_primaries']}/"
              f"{s['color_transfer']} {s['color_range']} {s['r_frame_rate']}")
        print(f"  {t['packets']} packets ({t['expected']} expected), steps "
              f"{t['min_step']:.6f}..{t['max_step']:.6f} s; {r['decoded_frames']} frames decoded")
        print(f"  error mean {r['error_mean']} p99 {r['error_p99']} max {r['error_max']} (8-bit luma); "
              f"{len(r['shifted_windows'])} of {r['windows']} windows shifted; "
              f"{r['repeat_count']} repeats in {r['moving_steps']} moving steps "
              f"({r['still_steps']} still); {r['wrong_count']} wrong; {r['torn_count']} torn")
        if a["present"]:
            loud = (f", {a['integrated_lufs']} LUFS, {a['true_peak_dbtp']} dBTP, "
                    f"{a['full_scale_samples']} at full scale" if "integrated_lufs" in a else "")
            print(f"  sound: {a['codec']} {a['sample_rate']} Hz x{a['channels']}, "
                  f"{a['seconds']:.4f} s against {a['video_seconds']:.4f} s "
                  f"({a['off_by_s'] * 1000:+.1f} ms){loud}")
        for p in a["problems"]:
            print(f"  sound: {p}")
        print(f"  -> {'ok' if r['ok'] else 'FAILED'}")
    (folder / f"{edit['cut']}.check.json").write_text(json.dumps(results, indent=2),
                                                     encoding="utf-8")
    sys.exit(0 if all(r["ok"] for r in results.values()) else 1)


if __name__ == "__main__":
    main()
