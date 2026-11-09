"""Cut the showcase film together from its captured takes (#437, #593).

    python tools/showcase/capture.py --cut master   # the takes a cut uses, at its size
    python tools/showcase/music.py master           # its score, to the same list
    python tools/showcase/cut.py master             # master MP4, web MP4 + WebM, poster
    python tools/showcase/check_cut.py master       # every frame, the sound, the bars
    python tools/showcase/cut.py reel               # reel.mp4 and its four stories

    python tools/showcase/cut.py master --dry-run         # the timeline; does the music fit it?
    python tools/showcase/cut.py master --still 2.0,40.5  # those film times as PNG, and stop
    python tools/showcase/cut.py reel --stills 6          # a few frames as PNG beside the film
    python tools/showcase/cut.py reel --guides            # the safe area over every frame
    python tools/showcase/cut.py master --scale 0.5       # takes captured with --scale 0.5
    python tools/showcase/cut.py master --web-only        # the web files again, from the master
    python tools/showcase/cut.py master --silent          # no soundtrack: a picture check

One editor for every cut. A cut is `tools/showcase/cuts/<name>.toml`, the
same file its music is scored to (music.py): a [cut] table (size, fps, the
bar grid, the titles' layout, the soundtrack) and its [[shot]]s in order,
each a take of a beat from `start_s` seconds for a number of bars. The last
shot's take runs on for `tail_bars` more, where the music rings out. This
reads the frames capture.py wrote for the cut
(`<out>/<cut>/<beat>/<take>/frame_NNNN.png`, frame 1 at t = 0), composes
the titles over them in a worker pool and pipes every frame once to ffmpeg,
which encodes the cut with its soundtrack (and, for the reel, each
story-length piece at the same time). Nothing is resampled in time: the
film has exactly the frames its shots name, each used once and in order.

Titles come in two layouts, chosen by the cut's `titles`:

- "caption" (the reel): a caption at the top of the safe area, a small
  label under it, held across the shots that share it.
- "lower_third" (the master): each beat's number, title and one line at the
  lower left, a centred `card` on a shot (the film's title), and a `tag`
  that cuts with its shot (each look's name).

Both end on [cut.end]'s card over the last bars, then black. Before a frame
is composed the music's report (`many-hands-<cut>.json`, beside the WAV) is
read: a track scored to another list, or one the report doesn't describe,
is refused, so the picture can't drift off its music again (#593).

Writes into `<out>/<cut>/edit/`:

    <file>.mp4               the cut (the master's name is its [encode] file)
    <cut>_story_N.mp4        the reel's story pieces
    many-hands-web.mp4/.webm and the poster, if the cut has a [web] table
    edit.json                the timeline as encoded: shots, files, music
    <cut>.frames.json        which source frame each film frame is
    <cut>.thumbs.npy         a small BT.709 luma thumbnail of each composed frame,
                             which check_cut.py decodes every file against

Needs Python 3.11+, Pillow, numpy and ffmpeg on PATH (8.0, the gyan.dev
full build: `winget install Gyan.FFmpeg`).
"""

import argparse
import collections
import hashlib
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

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from capture import load_cut  # noqa: E402

ROOT = HERE.parents[1]

# The cut's own file, piped from the frames, unless its [encode] says
# otherwise: H.264 High, BT.709, a keyframe a second, AAC.
ENCODE = {"crf": 16, "preset": "slow", "gop_s": 1.0, "audio_bitrate": "192k"}

# Colours of the lower thirds and cards (sRGB).
INK = (251, 247, 239)
INK_SOFT = (233, 227, 214)
ACCENT = (242, 201, 138)

# BT.709 on the frames themselves: ffmpeg 8 takes a stream's colour tags from
# its frames, and `-color_primaries` / `-color_trc` on the command line alone
# left primaries and transfer unset in #436's first master.
TAG_709 = "setparams=color_primaries=bt709:color_trc=bt709:colorspace=bt709:range=tv"


# --- The cut ---------------------------------------------------------------

def load(name, scale):
    """The cut's file, with its frame size (and safe area) scaled for takes
    captured at a fraction of it."""
    cut = load_cut(name)
    c = cut["cut"]
    c.setdefault("fps", 30)
    c.setdefault("titles", "caption")
    if scale != 1.0:
        c["width"] = int(round(c["width"] * scale))
        c["height"] = int(round(c["height"] * scale))
        if "safe" in c:
            c["safe"] = [int(round(v * scale)) for v in c["safe"]]
    if c["width"] % 2 or c["height"] % 2:
        raise SystemExit(f"{c['width']}x{c['height']}: yuv420p needs an even frame size")
    return cut


def bar_seconds(c):
    return c["beats_per_bar"] * 60.0 / c["bpm"]


def whole_frames(bars, c, what):
    n = bars * bar_seconds(c) * c["fps"]
    if abs(n - round(n)) > 1e-6:
        raise SystemExit(f"{what} is {n} frames: not a whole frame count")
    return int(round(n))


def build_timeline(cut, takes):
    """Every film frame's source PNG in order, and the shots with their
    frame ranges (`first`, `frames`; the last shot's `tail` frames follow)."""
    c = cut["cut"]
    fps = c["fps"]
    frames, shots = [], []
    last = len(cut["shot"]) - 1
    for i, shot in enumerate(cut["shot"]):
        what = f"shot {i} ({shot['beat']})"
        n = whole_frames(shot["bars"], c, what)
        tail = whole_frames(c.get("tail_bars", 0), c, "the tail") if i == last else 0
        take = shot.get("take", "main")
        folder = takes / shot["beat"] / take
        first = int(round(shot["start_s"] * fps)) + 1  # frame_0001 is t = 0
        paths = [folder / f"frame_{first + k:04d}.png" for k in range(n + tail)]
        missing = [p for p in (paths[0], paths[-1]) if not p.exists()]
        if missing:
            raise SystemExit(f"{what} needs {missing[0]}: python tools/showcase/capture.py "
                             f"--cut {c['name']}")
        # A take stepped at another rate would play at the wrong speed
        record = folder / "take.json"
        if record.exists():
            made = json.loads(record.read_text(encoding="utf-8"))
            if made.get("fps") != fps or (made.get("width"), made.get("height")) != (
                    c["width"], c["height"]):
                raise SystemExit(
                    f"{folder} was captured {made.get('width')}x{made.get('height')} at "
                    f"{made.get('fps')} fps; the cut is {c['width']}x{c['height']} at {fps}: "
                    f"python tools/showcase/capture.py --cut {c['name']} (or --scale here)")
        shots.append({**shot, "take": take, "index": i, "first": len(frames), "frames": n,
                      "tail": tail})
        frames += paths
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
    last = shots[-1]
    pieces.append((start, last["first"] + last["frames"] + last["tail"]))
    return pieces


# --- The music -------------------------------------------------------------

def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def music_report(cut, shots, total, audio):
    """The score's report beside the WAV, and what doesn't fit the picture:
    another cut's track, another length, a shot whose first frame isn't the
    music's, or a WAV the report doesn't describe."""
    c = cut["cut"]
    fps = c["fps"]
    path = audio.with_suffix(".json")
    if not path.exists():
        return None, [f"no {path.name} beside {audio.name}: nothing says which list it fits"]
    r = json.loads(path.read_text(encoding="utf-8"))
    problems = []
    if r.get("cut") != c["name"]:
        problems.append(f"it was scored to the {r.get('cut')!r} cut, not {c['name']!r}")
    want = total / fps * r["sample_rate"]
    if abs(r["samples"] - want) > 0.5:
        problems.append(f"it is {r['samples'] / r['sample_rate']:.3f} s and the picture "
                        f"{total / fps:.3f} s")
    scored = [s for s in r["shots"] if s["beat"] != "tail"]
    if len(scored) != len(shots):
        problems.append(f"it has {len(scored)} shots and the picture {len(shots)}")
    else:
        for m, s in zip(scored, shots):
            at = int(round(m["at_s"] * fps))
            if (m["beat"], m["take"]) != (s["beat"], s["take"]) or at != s["first"]:
                problems.append(f"shot {s['index']} ({s['beat']}/{s['take']}) starts on frame "
                                f"{s['first']}, the music's {m['beat']}/{m['take']} on {at}")
    if r.get("sha256") and sha256(audio) != r["sha256"]:
        problems.append(f"{audio.name} isn't the file {path.name} describes")
    return r, problems


# --- Type ------------------------------------------------------------------

def font_dirs():
    dirs = []
    if os.name == "nt":
        dirs.append(Path(os.environ.get("WINDIR", "C:/Windows")) / "Fonts")
        dirs.append(Path(os.environ.get("LOCALAPPDATA", "")) / "Microsoft" / "Windows" / "Fonts")
    dirs += [Path("/usr/share/fonts"), Path("/usr/local/share/fonts"), Path.home() / ".fonts",
             Path("/Library/Fonts"), Path("/System/Library/Fonts/Supplemental"),
             Path.home() / "Library" / "Fonts"]
    return dirs


def find_font(names):
    """The first of `names` found as given or in the system's font folders."""
    for name in names:
        if Path(name).is_file():
            return Path(name)
        for d in font_dirs():
            if not d.is_dir():
                continue
            if (d / name).is_file():
                return d / name
            hits = list(d.rglob(name))
            if hits:
                return hits[0]
    print(f"  no font of {names}; using Pillow's default", file=sys.stderr)
    return None


class Face:
    """A typeface that gives itself at any pixel size, cached."""

    def __init__(self, path):
        self.path = path
        self.sizes = {}

    def at(self, size):
        size = max(8, int(round(size)))
        if size not in self.sizes:
            self.sizes[size] = (ImageFont.truetype(str(self.path), size) if self.path
                                else ImageFont.load_default(size))
        return self.sizes[size]


def line_width(font, text, tracking=0.0):
    return sum(font.getlength(ch) for ch in text) + tracking * max(0, len(text) - 1)


def draw_tracked(draw, x, y, text, font, fill, tracking=0.0):
    if not tracking:
        draw.text((x, y), text, font=font, fill=fill)
        return
    for ch in text:
        draw.text((x, y), ch, font=font, fill=fill)
        x += font.getlength(ch) + tracking


def centred(draw, font, text, cx, y, fill, tracking=0.0):
    """Text centred on cx with its top at y; `tracking` spaces letters."""
    draw_tracked(draw, cx - line_width(font, text, tracking) / 2, y, text, font, fill, tracking)


def cropped(img, premultiply=True):
    """An RGBA image as (x, y, colour premultiplied 0..255, alpha 0..1),
    cropped to what is drawn: only that region is blended into a frame."""
    box = img.getchannel("A").getbbox()
    if box is None:
        return None
    a = np.asarray(img.crop(box), dtype=np.float32)
    alpha = a[..., 3:4] / 255.0
    rgb = a[..., :3] * alpha if premultiply else a[..., :3]
    return box[0], box[1], np.ascontiguousarray(rgb), np.ascontiguousarray(alpha)


# The reel's layout: a caption at the top of the safe area --------------------

class CaptionType:
    """The reel's faces and sizes, in pixels of a 1080-wide frame scaled to
    the cut's width. A line wider than the safe area shrinks to fit."""

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
        px = size * self.u
        while True:
            font = face.at(px)
            if line_width(font, text, tracking * self.u) <= self.room or px <= 8:
                return font, tracking * self.u
            px *= 0.96


def soft_shadow(size, draw_fn, radius, strength):
    """Text drawn by draw_fn in white over a blurred dark halo of itself,
    so a caption reads over sky and grass alike."""
    mask = Image.new("L", size, 0)
    draw_fn(ImageDraw.Draw(mask), 255)
    halo = mask.filter(ImageFilter.GaussianBlur(radius))
    halo = halo.point(lambda v: min(255, int(v * strength)))
    layer = Image.new("RGBA", size, (8, 6, 12, 0))
    layer.putalpha(halo)
    text = Image.new("RGBA", size, (255, 248, 236, 0))
    text.putalpha(mask)
    return cropped(Image.alpha_composite(layer, text))


def caption_layer(cut, ty, caption, label, title=False):
    c = cut["cut"]
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

    return soft_shadow((c["width"], c["height"]), draw, radius=14 * u, strength=1.7)


def caption_end_card(cut, ty):
    """The title in the middle of the safe area, its line under it, and the
    small lines at the safe area's foot."""
    c = cut["cut"]
    e = c["end"]
    x0, y0, x1, y1 = c["safe"]
    cx = (x0 + x1) / 2
    u = ty.u
    mid = (y0 + y1) / 2 - 160 * u
    title, title_track = ty.fit(ty.serif, ty.CARD, e["title"], tracking=6)
    line, _ = ty.fit(ty.serif, ty.CARD_LINE, e.get("line", ""))
    smalls = small_lines(e)
    small_fonts = [ty.fit(ty.mono, ty.SMALL, s, tracking=1.0) for s in smalls]

    def draw(d, a):
        centred(d, title, e["title"], cx, mid, a, title_track)
        centred(d, line, e.get("line", ""), cx, mid + title.size * 1.25, a)
        step = ty.SMALL * u * 1.5
        y = y1 - 40 * u - step * len(smalls)
        for text, (font, track) in zip(smalls, small_fonts):
            centred(d, font, text, cx, y, a, track)
            y += step

    return soft_shadow((c["width"], c["height"]), draw, radius=22 * u, strength=1.9)


def small_lines(e):
    s = e.get("small", [])
    return [s] if isinstance(s, str) else list(s)


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
    return cropped(img)


# The master's layout: lower thirds, cards and tags ---------------------------

class Fonts:
    """The master's faces, sized in points of the cut's canvas."""

    def __init__(self, cut):
        c = cut["cut"]
        self.s = c["width"] / c.get("points_wide", 1920)
        self.faces = {k: Face(find_font(v)) for k, v in cut.get("fonts", {}).items()}

    def get(self, kind, points):
        face = self.faces.get(kind) or Face(None)
        return face.at(points * self.s)


def text_height(font):
    asc, desc = font.getmetrics()
    return asc + desc


def finish_layer(img, s, scrim_pad, scrim_alpha, scrim_blur, shadow_blur=5.0, shadow_alpha=0.65):
    """A soft scrim and a drop shadow under the text in `img` (RGBA, text
    only), cropped to what's drawn."""
    alpha = img.getchannel("A")
    box = alpha.getbbox()
    if box is None:
        return None
    pad = int(scrim_pad * s)
    scrim = Image.new("L", img.size, 0)
    ImageDraw.Draw(scrim).rounded_rectangle(
        (box[0] - pad, box[1] - pad, box[2] + pad, box[3] + pad),
        radius=pad, fill=int(255 * scrim_alpha))
    scrim = scrim.filter(ImageFilter.GaussianBlur(scrim_blur * s))
    shadow = alpha.filter(ImageFilter.GaussianBlur(shadow_blur * s / 2))
    shadow = shadow.point(lambda v: int(v * shadow_alpha))
    under = Image.new("RGBA", img.size, (8, 6, 4, 0))
    under.putalpha(Image.fromarray(np.maximum(np.asarray(scrim), np.asarray(shadow))))
    return cropped(Image.alpha_composite(under, img))


def lower_third_layer(cut, fonts, number, total, title, caption):
    c = cut["cut"]
    size = (c["width"], c["height"])
    s = fonts.s
    img = Image.new("RGBA", size, (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    x = 96 * s
    bottom = size[1] - 92 * s
    mono, head, body = fonts.get("mono", 19), fonts.get("title", 50), fonts.get("body", 25)
    kicker = f"{number} / {total:02d}" if number else ""
    kh, th, bh = text_height(mono), text_height(head), text_height(body)
    y_cap = bottom - bh
    y_title = y_cap - th + 2 * s if caption else bottom - th
    y_kick = y_title - kh - 6 * s
    if kicker:
        d.rectangle((x, y_kick + kh * 0.45, x + 34 * s, y_kick + kh * 0.45 + 2 * s), fill=ACCENT)
        draw_tracked(d, x + 46 * s, y_kick, kicker, mono, ACCENT + (255,), 2.5 * s)
    d.text((x - 2 * s, y_title), title, font=head, fill=INK + (255,))
    if caption:
        d.text((x, y_cap), caption, font=body, fill=INK_SOFT + (240,))
    return finish_layer(img, s, scrim_pad=70, scrim_alpha=0.30, scrim_blur=64)


def card_layer(cut, fonts, title, line, smalls=()):
    """A centred card high in the frame: the title, a rule, its line, and
    any small lines under it."""
    c = cut["cut"]
    size = (c["width"], c["height"])
    s = fonts.s
    img = Image.new("RGBA", size, (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    head, body, mono = fonts.get("title_italic", 118), fonts.get("body", 31), fonts.get("mono", 20)
    cx = size[0] / 2
    cy = size[1] * 0.21
    centred(d, head, title, cx, cy - text_height(head), INK + (255,))
    rule = 56 * s
    d.rectangle((cx - rule / 2, cy + 16 * s, cx + rule / 2, cy + 18 * s), fill=ACCENT)
    y = cy + 34 * s
    if line:
        centred(d, body, line, cx, y, INK_SOFT + (245,))
        y += text_height(body) + 22 * s
    for text in smalls:
        centred(d, mono, text, cx, y, ACCENT + (235,), 1.5 * s)
        y += text_height(mono) * 1.35
    return finish_layer(img, s, scrim_pad=110, scrim_alpha=0.26, scrim_blur=100, shadow_blur=8)


def tag_layer(cut, fonts, tag):
    c = cut["cut"]
    s = fonts.s
    img = Image.new("RGBA", (c["width"], c["height"]), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    mono = fonts.get("mono", 22)
    w, h = line_width(mono, tag), text_height(mono)
    d.text((c["width"] - 96 * s - w, c["height"] - 92 * s - h), tag, font=mono,
           fill=ACCENT + (255,))
    return finish_layer(img, s, scrim_pad=26, scrim_alpha=0.40, scrim_blur=22)


# --- What is drawn when ----------------------------------------------------

def ramp(x):
    x = min(1.0, max(0.0, x))
    return x * x * (3.0 - 2.0 * x)


def overlay(spec, start, end, fade_in=0, fade_out=0, slide=0.0):
    """Something drawn over frames [start, end), fading in and out over
    that many frames; `slide` is the pixels it travels left of its place
    while fading in. `spec` says how to draw it (see build_layer)."""
    return {"spec": spec, "start": int(start), "end": int(end), "fade_in": int(fade_in),
            "fade_out": int(fade_out), "slide": float(slide)}


def runs(shots, key):
    """Runs of consecutive shots with the same key(shot): (first, end, shots)."""
    out = []
    for s in shots:
        end = s["first"] + s["frames"] + s["tail"]
        if out and key(out[-1][2][-1]) == key(s):
            out[-1] = (out[-1][0], end, out[-1][2] + [s])
        else:
            out.append((s["first"], end, [s]))
    return out


def caption_overlays(cut, shots, total, end_first):
    """The reel's: a caption held across the shots that share it, its label
    changing shot by shot, bowing out before the end card."""
    fps = cut["cut"]["fps"]
    fade = int(round(0.3 * fps))
    out = []
    for first, end, run in runs(shots, lambda s: s.get("caption")):
        text = run[0].get("caption")
        if not text:
            continue
        stop = min(end, end_first)
        title = run[0]["index"] == 0
        for k, s in enumerate(run):
            a = s["first"]
            b = min(stop, s["first"] + s["frames"] + s["tail"])
            if b <= a:
                continue
            out.append(overlay(("caption", text, s.get("label", ""), title), a, b,
                               fade if k == 0 else 0, fade if b == stop else 0))
    return out


def lower_third_overlays(cut, shots, titles):
    """The master's: a lower third per beat (shared by its shots in a row),
    a card where a shot has one, and each shot's tag."""
    c = cut["cut"]
    fps = c["fps"]
    lt = cut.get("lower_third", {})
    s = c["width"] / c.get("points_wide", 1920)
    beats = runs(shots, lambda x: x["beat"])
    total = len(beats)
    out = []
    for first, end, run in beats:
        head = run[0]
        if head.get("lower_third") is False:
            continue
        at = first + head.get("lower_third_at", lt.get("at", 0.6)) * fps
        stop = at + head.get("lower_third_hold", lt.get("hold", 4.2)) * fps
        if stop > end:
            raise SystemExit(f"{head['beat']}'s lower third runs to {stop / fps:.2f} s, past "
                             f"its shots' end at {end / fps:.2f} s: a shorter lower_third_hold")
        title = head.get("title") or titles.get(head["beat"], head["beat"])
        fade = round(lt.get("fade", 0.45) * fps)
        out.append(overlay(("lower_third", head["beat"][:2], total, title, head.get("caption", "")),
                           at, stop, fade, fade, lt.get("slide", 14.0) * s))
    for shot in shots:
        card = shot.get("card")
        if card:
            at = shot["first"] + card.get("at", 0.5) * fps
            fade = round(card.get("fade", 0.7) * fps)
            out.append(overlay(("card", card["title"], card.get("line", ""), ()),
                               at, at + card.get("hold", 3.0) * fps, fade, fade))
        if shot.get("tag"):
            # A tag cuts with its shot: no fade, exactly the shot's frames
            out.append(overlay(("tag", shot["tag"]), shot["first"],
                               shot["first"] + shot["frames"] + shot["tail"]))
    return out


def plan(cut, frames, shots, titles, guides=False):
    """For every film frame: its source, the overlays on it (index, opacity,
    slide in pixels), and how much the frame is darkened under them and
    faded after. Returns (plan, overlays)."""
    c = cut["cut"]
    fps = c["fps"]
    total = len(frames)
    e = c.get("end")
    end_frames = whole_frames(e["bars"], c, "the end card") if e else 0
    end_first = total - end_frames
    if c["titles"] == "caption":
        ovs = caption_overlays(cut, shots, total, end_first if e else total)
        end_spec = ("caption_end",)
    elif c["titles"] == "lower_third":
        ovs = lower_third_overlays(cut, shots, titles)
        end_spec = ("card", e["title"], e.get("line", ""), tuple(small_lines(e))) if e else None
    else:
        raise SystemExit(f"titles = {c['titles']!r}: \"caption\" or \"lower_third\"")
    if e:
        # The evening darkens under the card, then the card comes up
        ovs.append(overlay(end_spec, end_first + 0.3 * fps, total, 0.8 * fps, 0))
    if guides:
        ovs.append(overlay(("guides",), 0, total))
    fade_in = int(round(c.get("fade_in_s", 0.5) * fps))
    fade_out = int(round((e or c).get("fade_out_s", 1.0 if e else 0.0) * fps))
    darken = e.get("darken", 0.35) if e else 0.0

    out = []
    for f, src in enumerate(frames):
        on = []
        for k, o in enumerate(ovs):
            if not o["start"] <= f < o["end"]:
                continue
            k_in = ramp((f - o["start"] + 1) / o["fade_in"]) if o["fade_in"] else 1.0
            k_out = ramp((o["end"] - f) / o["fade_out"]) if o["fade_out"] else 1.0
            if min(k_in, k_out) > 0:
                on.append((k, min(k_in, k_out), -o["slide"] * (1.0 - k_in)))
        under = 1.0 - darken * ramp((f - end_first) / (0.8 * fps)) if e and f >= end_first else 1.0
        after = 1.0
        if fade_out and f >= total - fade_out:
            after *= ramp((total - f) / fade_out)
        if f < fade_in:
            after *= ramp(f / fade_in)
        out.append((str(src), on, under, after))
    return out, ovs


# --- Composing (in workers) ------------------------------------------------

W = {}  # per process: the cut, the plan, the overlays and their layers


def worker_init(cut, frame_plan, ovs):
    W.update(cut=cut, plan=frame_plan, overlays=ovs, layers={})
    if cut["cut"]["titles"] == "caption":
        W["type"] = CaptionType(cut)
    else:
        W["fonts"] = Fonts(cut)


def build_layer(spec):
    cut = W["cut"]
    kind = spec[0]
    if kind == "caption":
        return caption_layer(cut, W["type"], *spec[1:])
    if kind == "caption_end":
        return caption_end_card(cut, W["type"])
    if kind == "guides":
        return guides_layer(cut)
    if kind == "lower_third":
        return lower_third_layer(cut, W["fonts"], *spec[1:])
    if kind == "card":
        return card_layer(cut, W["fonts"], *spec[1:])
    if kind == "tag":
        return tag_layer(cut, W["fonts"], spec[1])
    raise ValueError(kind)


def layer(k):
    if k not in W["layers"]:
        W["layers"][k] = build_layer(tuple(W["overlays"][k]["spec"]))
    return W["layers"][k]


def blend(frame, lay, opacity, dx):
    """Lay a cropped premultiplied layer over a float frame, in place."""
    x, y, rgb, a = lay
    x += int(round(dx))
    h, w = a.shape[:2]
    x0, x1 = max(0, x), min(frame.shape[1], x + w)
    y0, y1 = max(0, y), min(frame.shape[0], y + h)
    if x1 <= x0 or y1 <= y0:
        return
    ax = a[y0 - y:y1 - y, x0 - x:x1 - x] * opacity
    region = frame[y0:y1, x0:x1]
    region *= 1.0 - ax
    region += rgb[y0 - y:y1 - y, x0 - x:x1 - x] * opacity


def compose(i):
    """Film frame i as RGB bytes, and its luma thumbnail."""
    src, on, under, after = W["plan"][i]
    c = W["cut"]["cut"]
    img = Image.open(src).convert("RGB")
    if img.size != (c["width"], c["height"]):
        raise SystemExit(f"{src} is {img.size[0]}x{img.size[1]}, the cut is "
                         f"{c['width']}x{c['height']}")
    if not on and under == 1.0 and after == 1.0:
        out = np.asarray(img, dtype=np.uint8)
    else:
        frame = np.asarray(img, dtype=np.float32)
        if under != 1.0:
            frame *= under
        for k, opacity, dx in on:
            lay = layer(k)
            if lay is not None:
                blend(frame, lay, opacity, dx)
        if after != 1.0:
            frame *= after
        out = np.clip(frame + 0.5, 0, 255).astype(np.uint8)
    return i, out.tobytes(), thumb(out)


def thumb_size(c):
    return (160, 90) if c["width"] >= c["height"] else (90, 160)


def thumb(rgb):
    """A 160x90 (90x160 upright) BT.709 luma thumbnail, by box averaging."""
    size = thumb_size(W["cut"]["cut"])
    small = np.asarray(Image.fromarray(rgb).resize(size, Image.BOX)).astype(np.float32)
    y = small @ np.array([0.2126, 0.7152, 0.0722], np.float32)
    return np.clip(y + 0.5, 0, 255).astype(np.uint8)


# --- Encoding --------------------------------------------------------------

def color_args():
    return ["-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709",
            "-color_range", "tv"]


def h264_level(w, h, fps):
    if w * h > 1920 * 1088:
        return "5.2" if fps > 30 else "5.1"
    return "4.2"


def pipe_cmd(cut, out, audio, start_frame, frames):
    """ffmpeg reading raw RGB frames on stdin, with `frames` of the
    soundtrack from start_frame (or silence), into an H.264/AAC MP4."""
    c = cut["cut"]
    enc = {**ENCODE, **cut.get("encode", {})}
    fps = c["fps"]
    seconds = frames / fps
    gop = str(max(1, int(round(enc["gop_s"] * fps))))
    cmd = ["ffmpeg", "-hide_banner", "-loglevel", "error", "-y",
           "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{c['width']}x{c['height']}",
           "-framerate", str(fps), "-i", "-"]
    if audio:
        cmd += ["-ss", f"{start_frame / fps:.6f}", "-t", f"{seconds:.6f}", "-i", str(audio)]
    else:
        cmd += ["-f", "lavfi", "-t", f"{seconds:.6f}",
                "-i", "anullsrc=channel_layout=stereo:sample_rate=48000"]
    cmd += [
        "-map", "0:v", "-map", "1:a",
        # sRGB frames into BT.709 limited-range YUV, tagged as such
        "-vf", "scale=out_color_matrix=bt709:out_range=tv:flags=lanczos+accurate_rnd"
               "+full_chroma_int,format=yuv420p," + TAG_709,
        "-c:v", "libx264", "-preset", enc["preset"], "-crf", str(enc["crf"]),
        "-profile:v", "high", "-level:v", h264_level(c["width"], c["height"], fps),
        "-g", gop, "-keyint_min", gop, "-sc_threshold", "0", "-bf", "2",
    ]
    if enc.get("maxrate"):
        cmd += ["-maxrate", enc["maxrate"], "-bufsize", enc.get("bufsize", enc["maxrate"])]
    cmd += [*color_args(),
            "-c:a", "aac", "-b:a", enc["audio_bitrate"], "-ar", "48000",
            "-t", f"{seconds:.6f}", "-movflags", "+faststart", "-fps_mode", "passthrough",
            "-threads", "0", str(out)]
    return cmd


def web_size(cut):
    """The web files' size: [web]'s, or the cut's if that is smaller (a
    --scale preview isn't scaled up)."""
    c, web = cut["cut"], cut["web"]
    if c["width"] < web["width"]:
        return c["width"], c["height"]
    return web["width"], web["height"]


def run(cmd):
    if subprocess.run(cmd).returncode != 0:
        raise SystemExit(f"failed: {' '.join(cmd)}")


def encode_web(cut, master, audio, out_dir, which, seconds):
    """The page's MP4 (H.264/AAC) and WebM (VP9/Opus): the master's picture,
    scaled and at the web rate, with the soundtrack from the WAV itself (not
    the master's AAC again)."""
    web = cut["web"]
    fps = cut["cut"]["fps"]
    w, h = web_size(cut)
    vf = f"scale={w}:{h}:flags=lanczos"
    if web.get("fps", fps) != fps:
        vf += f",fps={web['fps']}"
    vf += ",format=yuv420p," + TAG_709
    # A keyframe every 5 s: the glyphs make keyframes dear (2 s cost a third
    # more bytes), and a page film is seeked rarely
    gop = str(web.get("fps", fps) * 5)
    abr = web.get("audio_bitrate", "160k")
    inputs = ["-i", str(master)] + (["-i", str(audio)] if audio else [])
    maps = ["-map", "0:v"] + (["-map", "1:a"] if audio else [])
    t = ["-t", f"{seconds:.6f}"]
    files = {}
    if "mp4" in which:
        mp4 = out_dir / web["mp4"]
        run(["ffmpeg", "-hide_banner", "-loglevel", "warning", "-y", *inputs, *maps,
             "-vf", vf, "-c:v", "libx264", "-preset", "slow", "-crf", str(web["mp4_crf"]),
             "-profile:v", "high", "-pix_fmt", "yuv420p", "-g", gop, *color_args(),
             *(["-c:a", "aac", "-b:a", abr, "-ar", "48000"] if audio else ["-an"]),
             *t, "-movflags", "+faststart", str(mp4)])
        files["mp4"] = mp4
    if "webm" in which:
        webm = out_dir / web["webm"]
        log = out_dir / "vp9pass"
        common = ["-vf", vf, "-c:v", "libvpx-vp9", "-b:v", web["webm_bitrate"], "-row-mt", "1",
                  "-tile-columns", "2", "-deadline", "good", "-g", gop, "-pix_fmt", "yuv420p",
                  *color_args(), "-passlogfile", str(log)]
        # pass 1 writes no file, which ffmpeg warns about: errors only
        run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-i", str(master),
             "-map", "0:v", *common, "-cpu-used", "4", "-pass", "1", "-an", *t,
             "-f", "null", os.devnull])
        run(["ffmpeg", "-hide_banner", "-loglevel", "warning", "-y", *inputs, *maps, *common,
             "-cpu-used", "2", "-pass", "2",
             *(["-c:a", "libopus", "-b:a", abr, "-ar", "48000"] if audio else ["-an"]),
             *t, str(webm)])
        for leftover in out_dir.glob("vp9pass*"):
            leftover.unlink()
        files["webm"] = webm
    return files


def make_poster(cut, takes, out_dir):
    web = cut["web"]
    src = takes / web["poster_beat"] / web.get("poster_take", "main")
    n = int(round(web["poster_time"] * cut["cut"]["fps"])) + 1
    img = Image.open(src / f"frame_{n:04d}.png").convert("RGB")
    img = img.resize(web_size(cut), Image.LANCZOS)
    path = out_dir / web["poster"]
    img.save(path, quality=88, optimize=True, progressive=True)
    return path


# --- Main ------------------------------------------------------------------

def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0],
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("cut", help="a cut in tools/showcase/cuts/ (master, reel)")
    ap.add_argument("--frames", help="the cut's takes (default <film out>/<cut>)")
    ap.add_argument("--out", help="where the films go (default <frames>/edit)")
    ap.add_argument("--audio", help="the soundtrack (default the cut's `audio`)")
    ap.add_argument("--silent", action="store_true", help="cut without a soundtrack")
    ap.add_argument("--scale", type=float, default=1.0,
                    help="the takes were captured at this fraction of the cut's size")
    ap.add_argument("--width", type=int, help="the takes' width (sets --scale)")
    ap.add_argument("--guides", action="store_true", help="draw the safe area over every frame")
    ap.add_argument("--stills", type=int, default=0,
                    help="also write this many frames, evenly spread, as PNG")
    ap.add_argument("--still", help="comma-separated film times: write those frames as PNG "
                    "and stop")
    ap.add_argument("--no-stories", action="store_true", help="the whole cut only")
    ap.add_argument("--no-web", action="store_true", help="skip the web files and poster")
    ap.add_argument("--web-only", action="store_true",
                    help="the web files and poster again, from the master already cut")
    ap.add_argument("--web-files", default="mp4,webm", help="which web files (mp4, webm)")
    ap.add_argument("--workers", type=int, default=max(2, min(8, (os.cpu_count() or 4) // 3)))
    ap.add_argument("--dry-run", action="store_true", help="print the timeline and stop")
    args = ap.parse_args()

    film_toml = tomllib.loads((HERE / "film.toml").read_text(encoding="utf-8"))
    film = film_toml["film"]
    titles = {b["id"]: b["title"] for b in film_toml["beat"]}
    base = load_cut(args.cut)["cut"]
    scale = args.width / base["width"] if args.width else args.scale
    cut = load(args.cut, scale)
    c = cut["cut"]
    fps = c["fps"]
    takes = Path(os.path.expanduser(args.frames or Path(film["out"]) / c["name"])).resolve()
    out_dir = Path(args.out).resolve() if args.out else takes / "edit"

    frames, shots = build_timeline(cut, takes)
    total = len(frames)
    seconds = total / fps
    pieces = [] if args.no_stories else stories(cut, shots)
    bar = bar_seconds(c)
    print(f"{c['name']}: {len(shots)} shots, {total} frames, {seconds:.2f} s "
          f"({seconds / bar:g} bars) at {c['width']}x{c['height']}, {fps} fps")
    for s in shots:
        tail = f" + {s['tail'] / fps:.1f} s tail" if s["tail"] else ""
        print(f"  {s['first'] / fps:6.2f} s  frame {s['first']:>5}  "
              f"{s['beat'] + '/' + s['take']:<26} from {s['start_s']:.2f} s, "
              f"{s['bars']:g} bars{tail}  {s.get('title') or s.get('caption', '') or s.get('tag', '')}")
    for k, (a, b) in enumerate(pieces, 1):
        print(f"  story {k}: {a / fps:.2f}-{b / fps:.2f} s ({(b - a) / fps:.2f} s)")
    if c.get("max_seconds") and seconds > c["max_seconds"]:
        raise SystemExit(f"{seconds:.1f} s is past the cut's {c['max_seconds']} s")

    # The soundtrack, and whether it was scored to this picture
    audio, music = None, None
    if not args.silent:
        given = args.audio or c.get("audio")
        if not given:
            raise SystemExit(f"{c['name']} has no `audio`: --audio <wav>, or --silent")
        audio = Path(os.path.expanduser(given))
        audio = audio if audio.is_absolute() else (ROOT / audio).resolve()
        if not audio.exists():
            raise SystemExit(f"no soundtrack at {audio}: python tools/showcase/music.py "
                             f"{c['name']} (or --silent)")
        music, problems = music_report(cut, shots, total, audio)
        if problems and not (args.audio and music is None):
            raise SystemExit(f"{audio.name} doesn't fit this picture:\n  " + "\n  ".join(problems)
                             + f"\nrebuild it: python tools/showcase/music.py {c['name']}")
        for p in problems:
            print(f"  warning: {p}", file=sys.stderr)
        if music:
            print(f"  music: {audio.name}, {music['integrated_lufs']} LUFS, "
                  f"{music['true_peak_dbtp']} dBTP; all {len(shots)} shots on its frames")
    if args.dry_run:
        return
    if not shutil.which("ffmpeg"):
        raise SystemExit("ffmpeg isn't on PATH (winget install Gyan.FFmpeg)")

    frame_plan, ovs = plan(cut, frames, shots, titles, guides=args.guides)
    out_dir.mkdir(parents=True, exist_ok=True)
    name = c["name"] + ("_guides" if args.guides else "")

    if args.still:
        worker_init(cut, frame_plan, ovs)
        for t in [float(x) for x in args.still.split(",")]:
            i = min(total - 1, int(round(t * fps)))
            _, data, _ = compose(i)
            p = out_dir / f"{name}_still_{t:07.2f}s.png"
            Image.frombytes("RGB", (c["width"], c["height"]), data).save(p)
            print(p)
        return

    record_path = out_dir / "edit.json"
    master = out_dir / cut.get("encode", {}).get("file", f"{name}.mp4")
    if args.web_only:
        if not master.exists() or not record_path.exists():
            raise SystemExit(f"no {master.name} to make the web files from: cut it first")
        record = json.loads(record_path.read_text(encoding="utf-8"))
    else:
        record = cut_master(cut, args, frames, frame_plan, ovs, pieces, master, audio,
                            out_dir, name)
    record.update(
        cut=c["name"], width=c["width"], height=c["height"], fps=fps, frames=total,
        seconds=round(seconds, 3), takes=str(takes),
        audio=str(audio) if audio else "silence",
        music=({"report": str(audio.with_suffix(".json")), "sha256": music["sha256"],
                "integrated_lufs": music["integrated_lufs"],
                "true_peak_dbtp": music["true_peak_dbtp"],
                "shots": [{k: m[k] for k in ("beat", "take", "at_s")} for m in music["shots"]]}
               if music else None),
        shots=[{k: s[k] for k in ("beat", "take", "start_s", "bars", "first", "frames", "tail")}
               | {k: s[k] for k in ("caption", "label", "title", "tag") if k in s}
               for s in shots],
        beats=[{"beat": run_[0]["beat"],
                "title": run_[0].get("title") or titles.get(run_[0]["beat"], ""),
                "first": first, "at_s": round(first / fps, 3)}
               for first, _, run_ in runs(shots, lambda x: x["beat"])],
    )
    if "safe" in c:
        record["safe"] = c["safe"]

    if cut.get("web") and not args.no_web:
        which = args.web_files.split(",")
        files = encode_web(cut, master, audio, out_dir, which, seconds)
        step = fps // cut["web"].get("fps", fps)
        record["files"] = [f for f in record.get("files", []) if f["key"] not in files]
        for key, path in files.items():
            record["files"].append({"key": key, "file": path.name, "first": 0,
                                    "frames": total // step, "step": step, "whole": True})
            print(f"wrote {path} ({path.stat().st_size / 1e6:.1f} MB)")
        poster = make_poster(cut, takes, out_dir)
        record["poster"] = poster.name
        print(f"wrote {poster}")
    record_path.write_text(json.dumps(record, indent=2), encoding="utf-8")
    print(f"wrote {record_path}; check it: python tools/showcase/check_cut.py {c['name']}")


def cut_master(cut, args, frames, frame_plan, ovs, pieces, master, audio, out_dir, name):
    """Compose every frame in the pool and pipe it to the cut's encoder(s).
    Returns the record's files, thumbs and stills."""
    c = cut["cut"]
    fps = c["fps"]
    total = len(frames)
    story_paths = [out_dir / f"{name}_story_{k}.mp4" for k in range(1, len(pieces) + 1)]
    whole = subprocess.Popen(pipe_cmd(cut, master, audio, 0, total), stdin=subprocess.PIPE)
    open_pieces = {}
    thumbs = np.zeros((total,) + thumb_size(c)[::-1], np.uint8)
    stills = set()
    if args.stills:
        stills = {int(round((k + 0.5) * total / args.stills)) for k in range(args.stills)}
    (out_dir / f"{c['name']}.frames.json").write_text(json.dumps({
        "fps": fps, "width": c["width"], "height": c["height"],
        "frames": [Path(p).relative_to(Path(p).parents[2]).as_posix() for p in frames],
    }), encoding="utf-8")

    print(f"  composing into {out_dir} with {args.workers} workers")
    started = time.time()
    with mp.Pool(args.workers, initializer=worker_init, initargs=(cut, frame_plan, ovs)) as pool:
        # A bounded window of frames in flight: a 4K frame is 25 MB of RGB
        pending = collections.deque()
        nxt = 0
        for f in range(total):
            while nxt < total and len(pending) < args.workers * 2:
                pending.append(pool.apply_async(compose, (nxt,)))
                nxt += 1
            i, buf, th = pending.popleft().get()
            assert i == f
            thumbs[f] = th
            for k, (a, b) in enumerate(pieces):
                if f == a:
                    open_pieces[k] = subprocess.Popen(
                        pipe_cmd(cut, story_paths[k], audio, a, b - a), stdin=subprocess.PIPE)
                if a <= f < b:
                    open_pieces[k].stdin.write(buf)
                if f == b - 1:
                    open_pieces[k].stdin.close()
            whole.stdin.write(buf)
            if f in stills:
                Image.frombytes("RGB", (c["width"], c["height"]), buf).save(
                    out_dir / f"{name}_still_{f / fps:06.2f}s.png")
            if f % (fps * 10) == 0 or f == total - 1:
                rate = (f + 1) / max(1e-6, time.time() - started)
                print(f"    {f / fps:6.1f} s  ({rate:.1f} frames/s)", flush=True)
    whole.stdin.close()
    failed = whole.wait() != 0
    for p in open_pieces.values():
        failed |= p.wait() != 0
    if failed:
        raise SystemExit("ffmpeg failed")
    np.save(out_dir / f"{c['name']}.thumbs.npy", thumbs)
    print(f"wrote {master} ({master.stat().st_size / 1e6:.1f} MB) in "
          f"{time.time() - started:.0f} s")
    files = [{"key": "master", "file": master.name, "first": 0, "frames": total, "step": 1,
              "whole": True}]
    for k, (p, (a, b)) in enumerate(zip(story_paths, pieces), 1):
        files.append({"key": f"story_{k}", "file": p.name, "first": a, "frames": b - a,
                      "step": 1, "whole": False})
        print(f"wrote {p}")
    return {"files": files, "thumbs": f"{c['name']}.thumbs.npy",
            "stories": [{"file": p.name, "first": a, "frames": b - a, "seconds": (b - a) / fps}
                        for p, (a, b) in zip(story_paths, pieces)],
            "file": master.name}


if __name__ == "__main__":
    main()
