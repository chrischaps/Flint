"""Cut the showcase film's captured takes into a finished film (#436).

    python tools/showcase/edit.py                     # cut_16x9.toml -> master, web, poster
    python tools/showcase/edit.py --still 2.0,40.5    # composed frames at those times, as PNG
    python tools/showcase/edit.py --master-only       # skip the web versions
    python tools/showcase/edit.py --web-only          # re-encode the web versions from the master
    python tools/showcase/edit.py --list              # the shots, their times and frames

The takes are PNG sequences written by capture.py (`<out>/<beat>/<take>/
frame_0001.png ...`). A cut file (`cut_16x9.toml`) lists shots, each a range
of one take, and the titles drawn over them. Every frame of the film is one
source frame, used once and in order, composed here (titles, fades) and
piped to ffmpeg as raw RGB, so nothing is resampled in time: the film has
exactly the frames the cut file names. Beside the film the edit writes

    <name>.frames.json   which source frame each film frame is
    <name>.thumbs.npy    a 160x90 BT.709 luma thumbnail of each composed frame

which `check_cut.py` decodes the encoded files against, frame by frame.

Needs Python 3.11+, Pillow, numpy and ffmpeg on PATH (8.0 was used: the
gyan.dev full build, `winget install Gyan.FFmpeg`).
"""

import argparse
import collections
import json
import multiprocessing as mp
import os
import shutil
import subprocess
import sys
import time
import tomllib
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont

ROOT = Path(__file__).resolve().parents[2]
HERE = ROOT / "tools" / "showcase"
POINTS_WIDE = 1920
THUMB = (160, 90)

# Colours of the titles (sRGB).
INK = (251, 247, 239)
INK_SOFT = (233, 227, 214)
ACCENT = (242, 201, 138)


# ------------------------------------------------------------------ plan


def load(cut_path):
    cut = tomllib.loads(Path(cut_path).read_text(encoding="utf-8"))
    film_toml = tomllib.loads((HERE / "film.toml").read_text(encoding="utf-8"))
    return cut, film_toml


def take_folder(root, beat, take):
    return root / beat / take


def plan_frames(cut, film_toml, root):
    """Every film frame: which source PNG, which shot, at what film time."""
    fps = cut["cut"]["fps"]
    film_fps = film_toml["film"]["fps"]
    if fps != film_fps:
        raise SystemExit(f"the cut is {fps} fps and the takes {film_fps}: recapture or change one")
    beats = {b["id"]: b for b in film_toml["beat"]}
    frames, shots = [], []
    for i, shot in enumerate(cut["shot"]):
        beat = beats.get(shot["beat"])
        if beat is None:
            raise SystemExit(f"shot {i + 1}: no beat {shot['beat']} in film.toml")
        take = shot.get("take", "main")
        folder = take_folder(root, shot["beat"], take)
        first = int(round(shot["in"] * fps)) + 1  # frame_0001 is t = 0
        count = int(round((shot["out"] - shot["in"]) * fps))
        last = first + count - 1
        have = len(list(folder.glob("frame_*.png"))) if folder.exists() else 0
        if have < last:
            raise SystemExit(
                f"shot {i + 1} ({shot['beat']}/{take}) needs frames {first}..{last} "
                f"and {folder} has {have}: python tools/showcase/capture.py {shot['beat']}"
            )
        start = len(frames)
        for n in range(first, last + 1):
            frames.append({"shot": i, "src": str(folder / f"frame_{n:04d}.png"), "n": n})
        shots.append({**shot, "take": take, "start": start, "frames": count,
                      "first": first, "last": last})
    return frames, shots


# ---------------------------------------------------------------- titles


def font_dirs():
    dirs = []
    if os.name == "nt":
        dirs.append(Path(os.environ.get("WINDIR", "C:/Windows")) / "Fonts")
        dirs.append(Path(os.environ.get("LOCALAPPDATA", "")) / "Microsoft" / "Windows" / "Fonts")
    dirs += [Path("/usr/share/fonts"), Path("/usr/local/share/fonts"),
             Path.home() / ".fonts", Path("/Library/Fonts"), Path.home() / "Library" / "Fonts"]
    return dirs


def find_font(names):
    for name in names:
        p = Path(name)
        if p.is_file():
            return p
        for d in font_dirs():
            if not d.exists():
                continue
            direct = d / name
            if direct.is_file():
                return direct
            hits = list(d.rglob(name))
            if hits:
                return hits[0]
    return None


class Fonts:
    def __init__(self, spec, scale):
        self.scale = scale
        self.paths = {k: find_font(v) for k, v in spec.items()}

    def get(self, kind, points):
        size = max(1, int(round(points * self.scale)))
        path = self.paths.get(kind)
        if path is None:
            return ImageFont.load_default(size)
        return ImageFont.truetype(str(path), size)


def text_size(draw, text, font, spacing=0.0):
    if spacing:
        w = sum(draw.textlength(c, font=font) for c in text) + spacing * (len(text) - 1)
    else:
        w = draw.textlength(text, font=font)
    asc, desc = font.getmetrics()
    return w, asc + desc


def draw_text(draw, xy, text, font, fill, spacing=0.0):
    x, y = xy
    if not spacing:
        draw.text((x, y), text, font=font, fill=fill)
        return
    for c in text:
        draw.text((x, y), c, font=font, fill=fill)
        x += draw.textlength(c, font=font) + spacing


def finish_layer(img, s, scrim_pad, scrim_alpha, scrim_blur, shadow_blur=5.0, shadow_alpha=0.65):
    """Put a soft scrim and a drop shadow under the text in `img` (RGBA,
    text only), crop it to what's drawn and return (x, y, rgb, alpha)."""
    alpha = img.getchannel("A")
    box = alpha.getbbox()
    if box is None:
        return None
    pad = int(scrim_pad * s)
    # Scrim: the text's box, grown and blurred into a soft dark cloud.
    scrim = Image.new("L", img.size, 0)
    ImageDraw.Draw(scrim).rounded_rectangle(
        (box[0] - pad, box[1] - pad, box[2] + pad, box[3] + pad),
        radius=pad, fill=int(255 * scrim_alpha))
    scrim = scrim.filter(ImageFilter.GaussianBlur(scrim_blur * s))
    shadow = alpha.filter(ImageFilter.GaussianBlur(shadow_blur * s / 2))
    shadow = shadow.point(lambda v: int(v * shadow_alpha))
    under = Image.new("RGBA", img.size, (8, 6, 4, 0))
    under.putalpha(Image.fromarray(np.maximum(np.asarray(scrim), np.asarray(shadow))))
    layer = Image.alpha_composite(under, img)
    full = layer.getchannel("A").getbbox()
    layer = layer.crop(full)
    arr = np.asarray(layer).astype(np.float32)
    return full[0], full[1], arr[..., :3], arr[..., 3:] / 255.0


def lower_third_layer(size, fonts, s, number, total, title, caption):
    img = Image.new("RGBA", size, (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    x = 96 * s
    bottom = size[1] - 92 * s
    mono = fonts.get("mono", 19)
    head = fonts.get("title", 50)
    body = fonts.get("body", 25)
    kicker = f"{number} / {total:02d}" if number else ""
    _, kh = text_size(d, kicker or "0", mono)
    _, th = text_size(d, title, head)
    _, bh = text_size(d, caption or "0", body)
    y_cap = bottom - bh
    y_title = y_cap - th + 2 * s if caption else bottom - th
    y_kick = y_title - kh - 6 * s
    if kicker:
        d.rectangle((x, y_kick + kh * 0.45, x + 34 * s, y_kick + kh * 0.45 + 2 * s), fill=ACCENT)
        draw_text(d, (x + 46 * s, y_kick), kicker, mono, ACCENT + (255,), spacing=2.5 * s)
    d.text((x - 2 * s, y_title), title, font=head, fill=INK + (255,))
    if caption:
        d.text((x, y_cap), caption, font=body, fill=INK_SOFT + (240,))
    return finish_layer(img, s, scrim_pad=70, scrim_alpha=0.30, scrim_blur=64)


def card_layer(size, fonts, s, title, caption):
    img = Image.new("RGBA", size, (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    head = fonts.get("title_italic", 118)
    body = fonts.get("body", 31)
    tw, th = text_size(d, title, head)
    cw, ch = text_size(d, caption, body)
    cy = size[1] * 0.21
    d.text(((size[0] - tw) / 2, cy - th), title, font=head, fill=INK + (255,))
    rule = 56 * s
    d.rectangle(((size[0] - rule) / 2, cy + 16 * s, (size[0] + rule) / 2, cy + 18 * s), fill=ACCENT)
    d.text(((size[0] - cw) / 2, cy + 34 * s), caption, font=body, fill=INK_SOFT + (245,))
    return finish_layer(img, s, scrim_pad=110, scrim_alpha=0.26, scrim_blur=100, shadow_blur=8)


def tag_layer(size, fonts, s, tag):
    img = Image.new("RGBA", size, (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    mono = fonts.get("mono", 22)
    w, h = text_size(d, tag, mono)
    d.text((size[0] - 96 * s - w, size[1] - 92 * s - h), tag, font=mono, fill=ACCENT + (255,))
    return finish_layer(img, s, scrim_pad=26, scrim_alpha=0.40, scrim_blur=22)


def overlays(cut, shots, fps):
    """What is drawn over the film and when, in film seconds: a list of
    dicts with kind, text, start, end (and fade / slide)."""
    lt = cut.get("lower_third", {})
    total = len({s["number"] for s in shots if s.get("number")})
    out = []
    for shot in shots:
        t0 = shot["start"] / fps
        if shot.get("title"):
            at = t0 + shot.get("lower_third_at", lt.get("at", 0.5))
            out.append({"kind": "lower_third", "number": shot.get("number", ""), "total": total,
                        "title": shot["title"], "caption": shot.get("caption", ""),
                        "start": at, "end": at + shot.get("lower_third_hold", lt.get("hold", 4.0)),
                        "fade": lt.get("fade", 0.45), "slide": lt.get("slide", 14.0)})
        if shot.get("card"):
            c = shot["card"]
            at = t0 + c.get("at", 0.5)
            out.append({"kind": "card", "title": c["title"], "caption": c.get("caption", ""),
                        "start": at, "end": at + c.get("hold", 3.0), "fade": c.get("fade", 0.7),
                        "slide": 0.0})
        if shot.get("tag"):
            # A tag cuts with its shot: no fade, exactly the shot's frames.
            out.append({"kind": "tag", "text": shot["tag"], "start": t0,
                        "end": t0 + shot["frames"] / fps, "fade": 0.0, "slide": 0.0})
    return out


def smooth(x):
    x = min(1.0, max(0.0, x))
    return x * x * (3 - 2 * x)


def strength(o, t):
    """How much of an overlay shows at film time t, and how far in it has slid."""
    if t < o["start"] or t >= o["end"]:
        return 0.0, 0.0
    if o["fade"] <= 0:
        return 1.0, 1.0
    k_in = smooth((t - o["start"]) / o["fade"])
    k_out = smooth((o["end"] - t) / o["fade"])
    return min(k_in, k_out), k_in


# ---------------------------------------------------------------- compose

W = {}  # per worker: plan, layers, sizes


def worker_init(cut, frames, overlay_list, size, fonts_spec):
    s = size[0] / POINTS_WIDE
    fonts = Fonts(fonts_spec, s)
    layers = []
    for o in overlay_list:
        if o["kind"] == "lower_third":
            lay = lower_third_layer(size, fonts, s, o["number"], o["total"], o["title"], o["caption"])
        elif o["kind"] == "card":
            lay = card_layer(size, fonts, s, o["title"], o["caption"])
        else:
            lay = tag_layer(size, fonts, s, o["text"])
        layers.append(lay)
    W.update(cut=cut, frames=frames, overlays=overlay_list, layers=layers, size=size, s=s,
             fps=cut["cut"]["fps"], total=len(frames))


def global_fade(i):
    c = W["cut"]["cut"]
    fps, total = W["fps"], W["total"]
    t = i / fps
    end = (total - 1 - i) / fps
    k = 1.0
    if c.get("fade_in", 0) > 0:
        k = min(k, smooth(t / c["fade_in"]))
    if c.get("fade_out", 0) > 0:
        k = min(k, smooth(end / c["fade_out"]))
    return k


def compose(i):
    """Film frame i: its source frame with the titles and fades drawn on."""
    f = W["frames"][i]
    img = Image.open(f["src"]).convert("RGB")
    if img.size != W["size"]:
        img = img.resize(W["size"], Image.LANCZOS)
    frame = np.asarray(img).astype(np.float32)
    t = i / W["fps"]
    for o, lay in zip(W["overlays"], W["layers"]):
        if lay is None:
            continue
        k, slid = strength(o, t)
        if k <= 0:
            continue
        x, y, rgb, a = lay
        x += int(round(-o["slide"] * W["s"] * (1 - slid)))
        h, w = a.shape[:2]
        x0, x1 = max(0, x), min(frame.shape[1], x + w)
        if x1 <= x0:
            continue
        region = frame[y:y + h, x0:x1]
        aa = a[:, x0 - x:x1 - x] * k
        region *= 1 - aa
        region += rgb[:, x0 - x:x1 - x] * aa
    fade = global_fade(i)
    if fade < 1:
        frame *= fade
    out = np.clip(frame + 0.5, 0, 255).astype(np.uint8)
    return i, out.tobytes(), thumb(out)


def thumb(rgb):
    """160x90 BT.709 luma (full range) of an RGB frame, by box averaging."""
    small = np.asarray(Image.fromarray(rgb).resize(THUMB, Image.BOX)).astype(np.float32)
    y = small @ np.array([0.2126, 0.7152, 0.0722], np.float32)
    return np.clip(y + 0.5, 0, 255).astype(np.uint8)


# ---------------------------------------------------------------- encode


# BT.709 on the frames themselves: ffmpeg 8 takes a stream's colour tags from
# its frames, and `-color_primaries` / `-color_trc` on the command line alone
# left primaries and transfer unset in the first master.
TAG_709 = "setparams=color_primaries=bt709:color_trc=bt709:colorspace=bt709:range=tv"


def color_args():
    return ["-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709",
            "-color_range", "tv"]


def master_command(cut, size, path):
    m = cut.get("master", {})
    fps = cut["cut"]["fps"]
    return [
        "ffmpeg", "-hide_banner", "-loglevel", "warning", "-y",
        "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{size[0]}x{size[1]}",
        "-framerate", str(fps), "-i", "-",
        "-vf", "scale=out_color_matrix=bt709:out_range=tv:flags=lanczos+accurate_rnd+full_chroma_int,format=yuv420p," + TAG_709,
        "-c:v", "libx264", "-preset", m.get("preset", "slow"), "-crf", str(m.get("crf", 14)),
        "-profile:v", "high", "-level:v", "5.2" if size[0] > 1920 else "4.2",
        "-g", str(fps // 2), "-keyint_min", str(fps // 2), "-sc_threshold", "0", "-bf", "2",
        *color_args(), "-movflags", "+faststart", "-fps_mode", "passthrough",
        str(path),
    ]


def render(cut, frames, overlay_list, size, out_path, workers, thumbs_path):
    cmd = master_command(cut, size, out_path)
    enc = subprocess.Popen(cmd, stdin=subprocess.PIPE)
    thumbs = np.zeros((len(frames), THUMB[1], THUMB[0]), np.uint8)
    started = time.time()
    with mp.Pool(workers, initializer=worker_init,
                 initargs=(cut, frames, overlay_list, size, cut.get("fonts", {}))) as pool:
        # A bounded window of frames in flight: a 4K frame is 25 MB of RGB.
        pending = collections.deque()
        nxt = 0
        written = 0
        while written < len(frames):
            while nxt < len(frames) and len(pending) < workers * 2:
                pending.append(pool.apply_async(compose, (nxt,)))
                nxt += 1
            i, data, th = pending.popleft().get()
            assert i == written
            enc.stdin.write(data)
            thumbs[i] = th
            written += 1
            if written % 300 == 0 or written == len(frames):
                rate = written / (time.time() - started)
                print(f"  {written}/{len(frames)} frames, {rate:.1f} fps", flush=True)
    enc.stdin.close()
    if enc.wait() != 0:
        raise SystemExit(f"ffmpeg failed: {' '.join(cmd)}")
    np.save(thumbs_path, thumbs)
    return time.time() - started


def encode_web(cut, master, out_dir, which=("mp4", "webm")):
    web = cut["web"]
    vf = f"scale={web['width']}:{web['height']}:flags=lanczos"
    src_fps = cut["cut"]["fps"]
    if web.get("fps", src_fps) != src_fps:
        vf += f",fps={web['fps']}"
    vf += ",format=yuv420p," + TAG_709
    mp4 = out_dir / web["mp4"]
    webm = out_dir / web["webm"]
    # A keyframe every 5 s: the glyphs make keyframes dear (2 s cost a third
    # more bytes), and a page film is seeked rarely.
    gop = str(web.get("fps", src_fps) * 5)
    if "mp4" in which:
        run(["ffmpeg", "-hide_banner", "-loglevel", "warning", "-y", "-i", str(master),
             "-vf", vf, "-c:v", "libx264", "-preset", "slow", "-crf", str(web["mp4_crf"]),
             "-profile:v", "high", "-pix_fmt", "yuv420p", "-g", gop, *color_args(),
             "-movflags", "+faststart", "-an", str(mp4)])
    for p in (1, 2) if "webm" in which else ():
        # pass 1 writes no file, which ffmpeg warns about: errors only
        run(["ffmpeg", "-hide_banner", "-loglevel", "error" if p == 1 else "warning", "-y",
             "-i", str(master),
             "-vf", vf, "-c:v", "libvpx-vp9", "-b:v", web["webm_bitrate"],
             "-row-mt", "1", "-tile-columns", "2", "-deadline", "good",
             "-cpu-used", "4" if p == 1 else "2", "-pass", str(p),
             "-passlogfile", str(out_dir / "vp9pass"), "-g", gop, "-pix_fmt", "yuv420p",
             *color_args(), "-an",
             *(["-f", "null", os.devnull] if p == 1 else [str(webm)])])
    for leftover in out_dir.glob("vp9pass*"):
        leftover.unlink()
    return mp4, webm


def make_poster(cut, root, out_dir):
    web = cut["web"]
    src = take_folder(root, web["poster_beat"], web.get("poster_take", "main"))
    n = int(round(web["poster_time"] * cut["cut"]["fps"])) + 1
    img = Image.open(src / f"frame_{n:04d}.png").convert("RGB")
    img = img.resize((web["width"], web["height"]), Image.LANCZOS)
    path = out_dir / web["poster"]
    img.save(path, quality=88, optimize=True, progressive=True)
    return path


def run(cmd):
    r = subprocess.run(cmd)
    if r.returncode != 0:
        raise SystemExit(f"failed: {' '.join(cmd)}")


def probe_size(path):
    return path.stat().st_size


# ---------------------------------------------------------------- main


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0],
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--cut", default=str(HERE / "cut_16x9.toml"))
    ap.add_argument("--takes", help="where the takes are (default: film.toml's [film] out)")
    ap.add_argument("--out", help="where the film goes (default: <takes>/cuts/<name>)")
    ap.add_argument("--still", help="comma-separated film times: write those frames as PNG and stop")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--master-only", action="store_true")
    ap.add_argument("--web-only", action="store_true")
    ap.add_argument("--web-files", default="mp4,webm",
                    help="which web files to encode (mp4, webm or both); the poster is always made")
    ap.add_argument("--workers", type=int, default=max(2, min(8, (os.cpu_count() or 4) // 3)))
    args = ap.parse_args()

    if shutil.which("ffmpeg") is None:
        raise SystemExit("ffmpeg is not on PATH (winget install Gyan.FFmpeg)")
    cut, film_toml = load(args.cut)
    root = Path(os.path.expanduser(args.takes or film_toml["film"]["out"])).resolve()
    name = cut["cut"]["name"]
    out_dir = Path(args.out).resolve() if args.out else root / "cuts" / name
    out_dir.mkdir(parents=True, exist_ok=True)
    size = (cut["cut"]["width"], cut["cut"]["height"])
    fps = cut["cut"]["fps"]

    frames, shots = plan_frames(cut, film_toml, root)
    overlay_list = overlays(cut, shots, fps)
    seconds = len(frames) / fps

    if args.list:
        for s in shots:
            t = s["start"] / fps
            print(f"{t:7.2f}s  {s['beat']}/{s['take']:<16} frames {s['first']:>4}..{s['last']:<4} "
                  f"({s['frames']})  {s.get('title', '') or s.get('tag', '')}")
        print(f"\n{len(frames)} frames, {seconds:.2f} s at {fps} fps, {size[0]}x{size[1]}")
        return

    if args.still:
        worker_init(cut, frames, overlay_list, size, cut.get("fonts", {}))
        for t in [float(x) for x in args.still.split(",")]:
            i = min(len(frames) - 1, int(round(t * fps)))
            _, data, _ = compose(i)
            p = out_dir / f"still_{t:07.2f}.png"
            Image.frombytes("RGB", size, data).save(p)
            print(p)
        return

    master = out_dir / cut["master"]["file"]
    report_path = out_dir / f"{name}.json"
    report = json.loads(report_path.read_text()) if report_path.exists() else {}

    if not args.web_only:
        print(f"{name}: {len(shots)} shots, {len(frames)} frames ({seconds:.2f} s) at "
              f"{size[0]}x{size[1]} {fps} fps -> {master}")
        (out_dir / f"{name}.frames.json").write_text(json.dumps({
            "fps": fps, "width": size[0], "height": size[1],
            "frames": [{"shot": f["shot"], "src": str(Path(f["src"]).relative_to(root)).replace("\\", "/")}
                       for f in frames],
            "shots": [{k: v for k, v in s.items() if k != "card"} for s in shots],
            "overlays": overlay_list,
        }, indent=1), encoding="utf-8")
        took = render(cut, frames, overlay_list, size, master, args.workers,
                      out_dir / f"{name}.thumbs.npy")
        report.update(master={"file": master.name, "bytes": probe_size(master),
                              "frames": len(frames), "seconds": round(seconds, 3),
                              "width": size[0], "height": size[1], "fps": fps,
                              "encode_seconds": round(took, 1)})
        report_path.write_text(json.dumps(report, indent=2), encoding="utf-8")
        print(f"  master {probe_size(master) / 1e6:.1f} MB in {took:.0f} s")
    elif "master" not in report:
        report["master"] = {"file": master.name, "bytes": probe_size(master), "frames": len(frames),
                            "seconds": round(seconds, 3), "width": size[0], "height": size[1], "fps": fps}

    if not args.master_only:
        started = time.time()
        mp4, webm = encode_web(cut, master, out_dir, args.web_files.split(","))
        poster = make_poster(cut, root, out_dir)
        web = cut["web"]
        report.update(web={"width": web["width"], "height": web["height"], "fps": web.get("fps", fps),
                           "mp4": {"file": mp4.name, "bytes": probe_size(mp4)},
                           "webm": {"file": webm.name, "bytes": probe_size(webm)},
                           "poster": {"file": poster.name, "bytes": probe_size(poster)},
                           "encode_seconds": round(time.time() - started, 1)})
        for p in (mp4, webm, poster):
            print(f"  {p.name}: {probe_size(p) / 1e6:.2f} MB")

    report_path.write_text(json.dumps(report, indent=2), encoding="utf-8")
    print(f"report: {report_path}")


if __name__ == "__main__":
    main()
