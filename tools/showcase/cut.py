"""Cut the showcase film together from its captured takes (#437).

    python tools/showcase/capture.py --cut reel    # the takes, 1080x1920
    python tools/showcase/cut.py reel             # reel.mp4 and its stories
    python tools/showcase/cut.py reel --audio track.wav   # with the music (#438)
    python tools/showcase/cut.py reel --guides    # draw the safe area, for review
    python tools/showcase/cut.py reel --stills 6  # also a few frames as PNG

A cut is `tools/showcase/cuts/<name>.toml`: a [cut] table (size, fps, the
bar grid, where captions may go) and its [[shot]]s in order, each a take
of a beat from `start_s` for a number of bars. This reads the frames
capture.py wrote for that cut (`<out>/<cut>/<beat>/<take>/frame_NNNN.png`,
frame 1 at t = 0), lays each shot's caption over it inside the cut's safe
area, adds the end card, and pipes every frame once to ffmpeg, which
encodes the whole cut and, at the same time, each story-length piece
(`story_bars`, split at shot boundaries) as its own file:

    <out>/<cut>/edit/<cut>.mp4
    <out>/<cut>/edit/<cut>_story_1.mp4 ...
    <out>/<cut>/edit/edit.json          # the timeline, as encoded

H.264 High, yuv420p (BT.709), 30 fps, a keyframe a second, AAC audio: the
track given with --audio (or the cut's `audio`), else silence, so the file
is the H.264/AAC MP4 a phone and Instagram expect.

Needs Python 3.11+, Pillow, numpy and ffmpeg on PATH.
"""

import argparse
import json
import os
import shutil
import subprocess
import sys
import tomllib
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from capture import load_cut  # noqa: E402

ROOT = HERE.parents[1]
MAX_REEL_SECONDS = 90.0  # what #437 promises; Instagram takes longer reels
FONT_DIRS = [
    Path(os.environ.get("WINDIR", "C:/Windows")) / "Fonts",
    Path.home() / "AppData/Local/Microsoft/Windows/Fonts",
    Path("/usr/share/fonts/truetype/dejavu"),
    Path("/Library/Fonts"),
    Path("/System/Library/Fonts/Supplemental"),
]


# --- The timeline ----------------------------------------------------------

def build_timeline(cut, film_out):
    """Every output frame in order: (source png, shot index), and the
    shots with their frame ranges."""
    c = cut["cut"]
    fps = c["fps"]
    bar = c["beats_per_bar"] * 60.0 / c["bpm"]
    frames, shots = [], []
    for i, shot in enumerate(cut["shot"]):
        n = shot["bars"] * bar * fps
        if abs(n - round(n)) > 1e-6:
            raise SystemExit(f"shot {i} ({shot['beat']}) is {n} frames: not a whole frame count")
        n = int(round(n))
        take = shot.get("take", "main")
        folder = film_out / shot["beat"] / take
        first = int(round(shot["start_s"] * fps)) + 1  # frame_0001 is t = 0
        paths = [folder / f"frame_{first + k:04d}.png" for k in range(n)]
        missing = [p for p in (paths[0], paths[-1]) if not p.exists()]
        if missing:
            raise SystemExit(
                f"shot {i} needs {missing[0]}: python tools/showcase/capture.py --cut {c['name']}")
        # A take stepped at another rate would play at the wrong speed
        record = folder / "take.json"
        if record.exists():
            made = json.loads(record.read_text(encoding="utf-8"))
            if made.get("fps") != fps or (made.get("width"), made.get("height")) != (
                    c["width"], c["height"]):
                raise SystemExit(
                    f"{folder} was captured {made.get('width')}x{made.get('height')} at "
                    f"{made.get('fps')} fps; the cut is {c['width']}x{c['height']} at {fps}: "
                    f"python tools/showcase/capture.py --cut {c['name']}")
        shots.append({**shot, "take": take, "index": i, "first": len(frames), "frames": n})
        frames += [(p, i) for p in paths]
    return frames, shots


def stories(cut, shots):
    """Story-length pieces: shots packed in order into runs of at most
    `story_bars`, each cut at a shot boundary. [(first frame, end frame)]."""
    limit = cut["cut"].get("story_bars")
    if not limit:
        return []
    pieces, start, bars = [], 0, 0.0
    for s in shots:
        if s["bars"] > limit:
            raise SystemExit(f"shot {s['index']} is longer than a story ({limit} bars)")
        if bars + s["bars"] > limit + 1e-9:
            pieces.append((start, s["first"]))
            start, bars = s["first"], 0.0
        bars += s["bars"]
    pieces.append((start, shots[-1]["first"] + shots[-1]["frames"]))
    return pieces


# --- Type ------------------------------------------------------------------

def find_font(names):
    """The first of `names` found in the usual font folders, or None."""
    for name in names:
        for d in FONT_DIRS:
            p = d / name
            if p.exists():
                return p
    print(f"  no font of {names}; using Pillow's default", file=sys.stderr)
    return None


class Face:
    """A typeface that gives itself at any size, cached."""

    def __init__(self, path):
        self.path = path
        self.sizes = {}

    def at(self, size):
        size = max(8, int(round(size)))
        if size not in self.sizes:
            self.sizes[size] = (ImageFont.truetype(str(self.path), size) if self.path
                                else ImageFont.load_default(size))
        return self.sizes[size]


class Type:
    """The cut's faces and its sizes, in pixels of a 1080-wide frame scaled
    to the cut's width. A line wider than the safe area shrinks to fit."""

    CAPTION = 72
    TITLE = 112
    LABEL = 32
    CARD = 156
    CARD_LINE = 50
    SMALL = 30

    def __init__(self, cut):
        c = cut["cut"]
        self.u = c["width"] / 1080.0
        x0, _, x1, _ = c["safe"]
        self.room = x1 - x0
        self.serif = Face(find_font(c.get("caption_font", ["georgiai.ttf"])))
        self.mono = Face(find_font(c.get("label_font", ["CascadiaMono.ttf"])))

    def fit(self, face, size, text, tracking=0.0):
        """`face` at `size` (1080-wide pixels), smaller if `text` would be
        wider than the safe area; and the tracking it was measured with."""
        px = size * self.u
        while True:
            font = face.at(px)
            if line_width(font, text, tracking * self.u) <= self.room or px <= 8:
                return font, tracking * self.u
            px *= 0.96


def line_width(font, text, tracking):
    return sum(font.getlength(ch) for ch in text) + tracking * max(0, len(text) - 1)


def soft_shadow(size, draw_fn, radius, strength):
    """An RGBA layer: text drawn by draw_fn in white over a blurred dark
    halo of itself, so a caption reads over sky and grass alike."""
    mask = Image.new("L", size, 0)
    draw_fn(ImageDraw.Draw(mask), 255)
    halo = mask.filter(ImageFilter.GaussianBlur(radius))
    halo = halo.point(lambda v: min(255, int(v * strength)))
    layer = Image.new("RGBA", size, (8, 6, 12, 0))
    layer.putalpha(halo)
    text = Image.new("RGBA", size, (255, 248, 236, 0))
    text.putalpha(mask)
    return Image.alpha_composite(layer, text)


def centred(draw, font, text, cx, y, fill, tracking=0.0):
    """Text centred on cx with its top at y; `tracking` spaces letters."""
    x = cx - line_width(font, text, tracking) / 2
    if not tracking:
        draw.text((x, y), text, font=font, fill=fill)
        return
    for ch in text:
        draw.text((x, y), ch, font=font, fill=fill)
        x += font.getlength(ch) + tracking


def caption_layer(cut, ty, caption, label, title=False):
    """A caption at the top of the safe area, its label under it."""
    c = cut["cut"]
    size = (c["width"], c["height"])
    x0, y0, x1, _ = c["safe"]
    cx = (x0 + x1) / 2
    u = ty.u
    font, _ = ty.fit(ty.serif, ty.TITLE if title else ty.CAPTION, caption)
    small, track = ty.fit(ty.mono, ty.LABEL, label or "", tracking=1.5)
    top = y0 + 26 * u

    def draw(d, a):
        centred(d, font, caption, cx, top, a)
        if label:
            centred(d, small, label, cx, top + font.size * 1.22 + 12 * u, a, track)

    return premultiplied(soft_shadow(size, draw, radius=14 * u, strength=1.7))


def end_card_layer(cut, ty):
    """The title in the middle of the safe area, its line under it, and
    the small lines (a string or a list, one a line) at the safe area's foot."""
    c = cut["cut"]
    e = c["end"]
    size = (c["width"], c["height"])
    x0, y0, x1, y1 = c["safe"]
    cx = (x0 + x1) / 2
    u = ty.u
    mid = (y0 + y1) / 2 - 160 * u
    title, title_track = ty.fit(ty.serif, ty.CARD, e["title"], tracking=6)
    line, _ = ty.fit(ty.serif, ty.CARD_LINE, e["line"])
    smalls = e.get("small", [])
    smalls = [smalls] if isinstance(smalls, str) else smalls
    small_fonts = [ty.fit(ty.mono, ty.SMALL, s, tracking=1.0) for s in smalls]

    def draw(d, a):
        centred(d, title, e["title"], cx, mid, a, title_track)
        centred(d, line, e["line"], cx, mid + title.size * 1.25, a)
        step = ty.SMALL * u * 1.5
        y = y1 - 40 * u - step * len(smalls)
        for text, (font, track) in zip(smalls, small_fonts):
            centred(d, font, text, cx, y, a, track)
            y += step

    return premultiplied(soft_shadow(size, draw, radius=22 * u, strength=1.9))


def premultiplied(img):
    """RGBA -> (colour premultiplied, alpha) float32 arrays, for blending."""
    a = np.asarray(img, dtype=np.float32) / 255.0
    alpha = a[..., 3:4]
    return a[..., :3] * alpha, alpha


def over(frame, layer, opacity):
    """frame (float 0..1 RGB) with a premultiplied layer laid over it."""
    if opacity <= 0.0:
        return frame
    rgb, alpha = layer
    return frame * (1.0 - alpha * opacity) + rgb * opacity


def guides_layer(cut):
    """The safe area and what Instagram lays over a reel, for review."""
    c = cut["cut"]
    w, h = c["width"], c["height"]
    x0, y0, x1, y1 = c["safe"]
    img = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    shade = (255, 40, 90, 60)
    d.rectangle([0, 0, w, y0], fill=shade)
    d.rectangle([0, y1, w, h], fill=shade)
    d.rectangle([0, y0, x0, y1], fill=shade)
    d.rectangle([x1, y0, w, y1], fill=shade)
    d.rectangle([x0, y0, x1, y1], outline=(255, 80, 120, 220), width=3)
    return premultiplied(img)


# --- Opacity over time -----------------------------------------------------

def ramp(x):
    x = min(1.0, max(0.0, x))
    return x * x * (3.0 - 2.0 * x)


def spans(shots):
    """Runs of consecutive shots that share a caption: (first, end, shots)."""
    out = []
    for s in shots:
        if out and out[-1][2][-1].get("caption") == s.get("caption"):
            out[-1] = (out[-1][0], s["first"] + s["frames"], out[-1][2] + [s])
        else:
            out.append((s["first"], s["first"] + s["frames"], [s]))
    return out


# --- Encoding --------------------------------------------------------------

def ffmpeg_cmd(c, out, audio, audio_start, frames):
    fps = c["fps"]
    cmd = [
        "ffmpeg", "-hide_banner", "-loglevel", "error", "-y",
        "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{c['width']}x{c['height']}",
        "-r", str(fps), "-i", "-",
    ]
    seconds = frames / fps
    if audio:
        cmd += ["-ss", f"{audio_start:.6f}", "-t", f"{seconds:.6f}", "-i", str(audio)]
    else:
        cmd += ["-f", "lavfi", "-t", f"{seconds:.6f}",
                "-i", "anullsrc=channel_layout=stereo:sample_rate=48000"]
    cmd += [
        # sRGB frames into BT.709 limited-range YUV, tagged as such
        "-vf", "scale=out_color_matrix=bt709:out_range=tv,format=yuv420p",
        "-c:v", "libx264", "-preset", "slow", "-crf", "16",
        "-profile:v", "high", "-level:v", "4.2",
        "-g", str(fps), "-keyint_min", str(fps), "-sc_threshold", "0",
        "-maxrate", "24M", "-bufsize", "48M",
        "-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709",
        "-color_range", "tv",
        "-c:a", "aac", "-b:a", "192k", "-ar", "48000",
        "-t", f"{seconds:.6f}",
        "-movflags", "+faststart",
        "-threads", "0",
        str(out),
    ]
    return cmd


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0],
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("cut", help="a cut in tools/showcase/cuts/ (reel)")
    ap.add_argument("--frames", help="the cut's captures (default <film out>/<cut>)")
    ap.add_argument("--out", help="where the MP4s go (default <frames>/edit)")
    ap.add_argument("--audio", help="the soundtrack (WAV, ...; default the cut's `audio`, "
                    "else silence)")
    ap.add_argument("--guides", action="store_true", help="draw the safe area over every frame")
    ap.add_argument("--stills", type=int, default=0,
                    help="also write this many frames, evenly spread, as PNG")
    ap.add_argument("--no-stories", action="store_true", help="the whole cut only")
    ap.add_argument("--dry-run", action="store_true", help="print the timeline and stop")
    args = ap.parse_args()

    film = tomllib.loads((HERE / "film.toml").read_text(encoding="utf-8"))["film"]
    cut = load_cut(args.cut)
    c = cut["cut"]
    fps = c["fps"]
    film_out = Path(os.path.expanduser(args.frames or Path(film["out"]) / c["name"])).resolve()
    out_dir = Path(args.out).resolve() if args.out else film_out / "edit"
    audio = args.audio or c.get("audio")
    if audio:
        audio = Path(os.path.expanduser(audio))
        if not audio.is_absolute():
            audio = (ROOT / audio).resolve()
        if not audio.exists():
            if args.audio:
                raise SystemExit(f"no soundtrack at {audio}")
            # The cut's own track may not be built yet: say so, cut silent
            print(f"  no soundtrack at {audio} (python tools/showcase/music.py "
                  f"{c['name']} builds it); the cut will be silent", file=sys.stderr)
            audio = None

    frames, shots = build_timeline(cut, film_out)
    total = len(frames)
    seconds = total / fps
    pieces = [] if args.no_stories else stories(cut, shots)
    bar = c["beats_per_bar"] * 60.0 / c["bpm"]
    print(f"{c['name']}: {len(shots)} shots, {total} frames, {seconds:.2f} s "
          f"({seconds / bar:g} bars) at {c['width']}x{c['height']}, {fps} fps")
    for s in shots:
        print(f"  {s['first'] / fps:6.2f} s  {s['beat'] + '/' + s['take']:<28} "
              f"from {s['start_s']:.2f} s, {s['bars']:g} bars  {s.get('caption', '')}")
    for k, (a, b) in enumerate(pieces, 1):
        print(f"  story {k}: {a / fps:.2f}-{b / fps:.2f} s ({(b - a) / fps:.2f} s)")
    if seconds > MAX_REEL_SECONDS:
        raise SystemExit(f"{seconds:.1f} s is past the reel's {MAX_REEL_SECONDS:.0f} s")
    if args.dry_run:
        return
    if not shutil.which("ffmpeg"):
        raise SystemExit("ffmpeg isn't on PATH")

    ty = Type(cut)
    print(f"  type: {ty.serif.path} / {ty.mono.path}")
    layers = {}

    def layer_for(caption, label, title):
        key = (caption, label, title)
        if key not in layers:
            layers[key] = caption_layer(cut, ty, caption, label, title)
        return layers[key]

    end = c.get("end")
    end_layer = end_card_layer(cut, ty) if end else None
    end_frames = int(round(end["bars"] * bar * fps)) if end else 0
    end_first = total - end_frames
    fade_out = int(round(end.get("fade_out_s", 1.0) * fps)) if end else 0
    guides = guides_layer(cut) if args.guides else None
    caption_fade = int(round(0.3 * fps))
    label_fade = max(2, int(round(0.08 * fps)))
    fade_in = int(round(0.5 * fps))

    # Opacity of each caption state on every frame: (layer key, opacity) pairs
    states = [[] for _ in range(total)]
    for first, end_at, run in spans(shots):
        text = run[0].get("caption")
        if not text:
            continue
        title = run[0]["index"] == 0
        # A caption over the end card's bars bows out before the card
        stop = min(end_at, end_first) if end else end_at
        for f in range(first, stop):
            o = ramp((f - first + 1) / caption_fade) * ramp((stop - f) / caption_fade)
            # The label may change shot by shot inside one caption: cross it
            s = next(x for x in run if x["first"] <= f < x["first"] + x["frames"])
            states[f].append(((text, s.get("label", ""), title), o))
    stills = set()
    if args.stills:
        stills = {int(round((k + 0.5) * total / args.stills)) for k in range(args.stills)}

    out_dir.mkdir(parents=True, exist_ok=True)
    name = c["name"] + ("_guides" if args.guides else "")
    whole = out_dir / f"{name}.mp4"
    encoders = [(0, total, whole, subprocess.Popen(
        ffmpeg_cmd(c, whole, audio, 0.0, total), stdin=subprocess.PIPE))]
    story_paths = []
    open_pieces = {}
    for k, (a, b) in enumerate(pieces, 1):
        story_paths.append(out_dir / f"{name}_story_{k}.mp4")

    print(f"  composing into {out_dir}")
    for f, (path, _) in enumerate(frames):
        img = np.asarray(Image.open(path).convert("RGB"), dtype=np.float32) / 255.0
        if img.shape[:2] != (c["height"], c["width"]):
            raise SystemExit(f"{path} is {img.shape[1]}x{img.shape[0]}, the cut is "
                             f"{c['width']}x{c['height']}")
        for key, o in states[f]:
            img = over(img, layer_for(*key), o)
        if end:
            if f >= end_first:
                # The evening darkens under the card, then the card comes up
                k = ramp((f - end_first) / (0.8 * fps))
                img = img * (1.0 - 0.35 * k)
                img = over(img, end_layer, ramp((f - end_first - 0.3 * fps) / (0.8 * fps)))
            img = img * ramp((total - f) / max(1, fade_out)) if f >= total - fade_out else img
        if f < fade_in:
            img = img * ramp(f / fade_in)
        if guides:
            img = over(img, guides, 1.0)
        raw = np.clip(img * 255.0 + 0.5, 0, 255).astype(np.uint8)
        buf = raw.tobytes()
        for k, (a, b) in enumerate(pieces):
            if f == a:
                open_pieces[k] = subprocess.Popen(
                    ffmpeg_cmd(c, story_paths[k], audio, a / fps, b - a), stdin=subprocess.PIPE)
            if a <= f < b:
                open_pieces[k].stdin.write(buf)
            if f == b - 1:
                open_pieces[k].stdin.close()
        encoders[0][3].stdin.write(buf)
        if f in stills:
            Image.fromarray(raw).save(out_dir / f"{name}_still_{f / fps:05.2f}s.png")
        if f % (fps * 6) == 0:
            print(f"    {f / fps:5.1f} s")
    encoders[0][3].stdin.close()
    failed = False
    for p in [encoders[0][3]] + list(open_pieces.values()):
        failed |= p.wait() != 0
    if failed:
        raise SystemExit("ffmpeg failed")

    record = {
        "cut": c["name"],
        "width": c["width"], "height": c["height"], "fps": fps,
        "frames": total, "seconds": round(seconds, 3),
        "audio": str(audio) if audio else "silence",
        "safe": c["safe"],
        "shots": [{k: s[k] for k in ("beat", "take", "start_s", "bars", "first", "frames")}
                  | {"caption": s.get("caption", ""), "label": s.get("label", "")}
                  for s in shots],
        "stories": [{"file": p.name, "first": a, "frames": b - a, "seconds": (b - a) / fps}
                    for p, (a, b) in zip(story_paths, pieces)],
        "file": whole.name,
    }
    (out_dir / "edit.json").write_text(json.dumps(record, indent=2), encoding="utf-8")
    print(f"wrote {whole}")
    for p in story_paths:
        print(f"wrote {p}")


if __name__ == "__main__":
    main()
