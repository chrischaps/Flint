"""*Many Hands*, the showcase film's music (#438): one tune, many hands.

The film redraws one clearing in many styles; the music plays one melody
in many instruments. The theme (an eight-bar phrase in D major, A, and an
eight-bar answer, B) runs straight through a cut on one clock, 100 bpm in
4/4, and each shot plays it in its own voice: the plain render on an
electric piano, the type beats on pulse waves whose width changes with
each ASCII table, edges as plucked lines, pencil as dry strings over the
scratch of a pencil trembling at the boil's 6 Hz, watercolor on a breathy
flute, the user styles in FM glass, the looks a new instrument every take.
At the end every hand plays a bar of it, and it comes home to D as the
camera rises into the evening; the crickets that opened the film keep on
after it.

`arrange(cut)` turns a cut list (tools/showcase/cuts/<cut>.toml) into six
stems and the mix automation that flint-music plays them with:

  foundation   bass and a soft heartbeat kick
  harmony      the pad
  world_voice  arpeggios: the texture of the beat's style
  home_theme   the melody
  child_motif  the figure's three-note call, in glass
  texture      crickets, glyph flicker, pencil scratch, paper, menu clicks

Cues inside a shot are in the take's own seconds (the storyboard's: 02's
table every 2.4 s, 03's edges at 2.5 s, 08/09's whole-frame switch at
5.0 s, 06's story steps, 07's blends, 05/12's clicks read from the input
scripts), so a moved in-point moves the cue with the picture.
"""

from __future__ import annotations

import zlib
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np

import synth
from synth import SR, Stem, midi_hz

BUSES = ["foundation", "harmony", "world_voice", "home_theme", "child_motif", "texture"]

# --- the theme ----------------------------------------------------------------
# (beat in the phrase, midi note, beats). D5 = 74.

MELODY = {
    "A": [
        (0, 69, 1), (1, 66, .5), (1.5, 69, .5), (2, 74, 2),
        (4, 73, 1.5), (5.5, 71, .5), (6, 69, 1), (7, 66, 1),
        (8, 67, 1), (9, 71, 1), (10, 78, 1.5), (11.5, 76, .5),
        (12, 74, 1), (13, 76, 1), (14, 69, 2),
        (16, 69, 1), (17, 66, .5), (17.5, 69, .5), (18, 74, 1), (19, 76, 1),
        (20, 78, 1.5), (21.5, 76, .5), (22, 73, 1), (23, 69, 1),
        (24, 71, 1), (25, 74, 1), (26, 79, 1), (27, 78, .5), (27.5, 76, .5),
        (28, 76, 1.5), (29.5, 73, .5), (30, 74, 2),
    ],
    "B": [
        (0, 71, 1.5), (1.5, 74, .5), (2, 78, 2),
        (4, 76, 1), (5, 73, 1), (6, 78, 1), (7, 76, 1),
        (8, 73, 1.5), (9.5, 69, .5), (10, 76, 2),
        (12, 74, 1), (13, 73, 1), (14, 71, 1), (15, 69, 1),
        (16, 67, 1), (17, 71, 1), (18, 78, 1.5), (19.5, 79, .5),
        (20, 76, 1), (21, 73, 1), (22, 69, 2),
        (24, 71, 1), (25, 74, .5), (25.5, 76, .5), (26, 78, 1), (27, 74, 1),
        (28, 76, 2), (30, 73, 1), (31, 76, 1),
    ],
}

# Pad voicings (no root: the bass has it) and bass roots.
V = {
    "Dmaj7": [57, 61, 62, 66], "Bm9": [59, 61, 62, 66], "Gmaj7": [55, 59, 62, 66],
    "A7sus4": [57, 62, 64, 67], "A7": [57, 61, 64, 67], "F#m7": [57, 61, 64, 66],
    "Em9": [55, 59, 62, 66], "A6": [57, 61, 64, 66], "Bm7": [57, 59, 62, 66],
    "Dmaj9": [57, 61, 62, 64, 66],
}
ROOT = {"Dmaj7": 38, "Bm9": 35, "Gmaj7": 43, "A7sus4": 33, "A7": 33, "F#m7": 42,
        "Em9": 40, "A6": 33, "Bm7": 35, "Dmaj9": 38}

# Per bar of the phrase: [(beat in bar, chord, beats)].
CHORDS = {
    "A": [[(0, "Dmaj7", 4)], [(0, "Bm9", 4)], [(0, "Gmaj7", 4)],
          [(0, "A7sus4", 2), (2, "A7", 2)], [(0, "Dmaj7", 4)], [(0, "F#m7", 4)],
          [(0, "Gmaj7", 4)], [(0, "Em9", 2), (2, "A7", 2)]],
    "B": [[(0, "Gmaj7", 4)], [(0, "A6", 4)], [(0, "F#m7", 4)], [(0, "Bm7", 4)],
          [(0, "Em9", 4)], [(0, "F#m7", 4)], [(0, "Gmaj7", 4)],
          [(0, "A7sus4", 2), (2, "A7", 2)]],
}
FORM = ["A", "A", "B", "A", "B", "A", "A", "B"]

# The figure's call, in beats from a bar's start.
MOTIF = [(2, 90, .5), (2.5, 88, .5), (3, 81, 1.5)]
MOTIF_HOME = [(2, 86, .5), (2.5, 88, .5), (3, 90, 1), (4, 93, 3)]  # rising, the last bar

BEAT_S = 0.6  # 100 bpm
BAR = 4


# --- the cut ------------------------------------------------------------------


@dataclass
class Shot:
    beat: str
    take: str
    start_s: float
    start: float  # beats from the cut's start
    length: float  # beats
    caption: str = ""

    @property
    def family(self) -> str:
        return FAMILY.get(self.beat, "plain")

    def take_time(self, beat: float) -> float:
        return self.start_s + (beat - self.start) * BEAT_S


FAMILY = {
    "01_plain": "plain", "02_type": "type", "03_edges": "edges", "04_entities": "entities",
    "05_mask": "mask", "06_story": "story", "07_reveal": "reveal", "08_pencil": "pencil",
    "09_watercolor": "watercolor", "10_looks": "looks", "11_user": "user", "12_menu": "menu",
    "13_hands": "hands", "tail": "coda",
}


@dataclass
class Cut:
    name: str
    shots: list[Shot]
    bars: float
    tail_bars: float
    raw: dict = field(default_factory=dict)

    @property
    def beats(self) -> float:
        return self.bars * BAR

    @property
    def seconds(self) -> float:
        return self.beats * BEAT_S

    def shot_at(self, beat: float) -> Shot:
        for s in self.shots:
            if s.start <= beat + 1e-9 < s.start + s.length:
                return s
        return self.shots[-1]


def load_cut(path: Path) -> Cut:
    import tomllib
    d = tomllib.loads(path.read_text(encoding="utf-8"))
    c = d["cut"]
    if c.get("bpm", 100) != 100 or c.get("beats_per_bar", 4) != 4:
        raise SystemExit(f"{path}: the score is written at 100 bpm in 4/4")
    shots, at = [], 0.0
    for s in d["shot"]:
        beats = float(s["bars"]) * BAR
        if abs(beats * 4 - round(beats * 4)) > 1e-9:
            raise SystemExit(f"{path}: shot {s['beat']}/{s.get('take')} is not a whole number "
                             "of sixteenths")
        shots.append(Shot(s["beat"], s.get("take", "main"), float(s.get("start_s", 0.0)), at,
                          beats, s.get("caption", "")))
        at += beats
    tail = float(c.get("tail_bars", 0))
    if tail > 0:
        last = shots[-1]
        shots.append(Shot("tail", "hold", last.take_time(at), at, tail * BAR))
        at += tail * BAR
    bars = at / BAR
    if abs(bars - round(bars)) > 1e-9:
        raise SystemExit(f"{path}: the cut is {bars} bars; it must end on a bar line")
    return Cut(c["name"], shots, round(bars), tail, d)


# --- the arrangement ----------------------------------------------------------


def _phrase(bar: int) -> tuple[str, int]:
    return FORM[(bar // 8) % len(FORM)], bar % 8


def _clicks(beat_id: str) -> list[float]:
    """Mouse clicks and key taps of a beat's input script, in take seconds."""
    path = Path(__file__).resolve().parents[2] / "demo/film/beats" / f"{beat_id}.input.txt"
    out = []
    if path.exists():
        for line in path.read_text(encoding="utf-8").splitlines():
            parts = line.split("#")[0].split()
            if len(parts) >= 2 and parts[1] in ("click", "tap", "down", "up"):
                out.append(float(parts[0]))
    return out


def _pulse(t: float, start: float, rise: float, end: float, fall: float) -> float:
    up = np.clip((t - start) / rise, 0, 1)
    down = np.clip((end - t) / fall, 0, 1)
    return float(min(up, down))


def _reveal_blend(t: float) -> float:
    return max(_pulse(t, 0.8, 3.0, 9.4, 2.8), _pulse(t, 2.6, 2.4, 9.4, 2.4))


# A look's hand: (melody voice, its arguments, octave, arpeggio voice or None,
# arpeggio arguments, arpeggio steps a beat, texture bed).
LOOKS = {
    "ascii_noir": ("pulse", {"duty": 0.5, "bright": 0.45}, -12, None, {}, 0, "flicker"),
    "matrix": ("pulse", {"duty": 0.125}, 0, "pulse", {"duty": 0.125, "vel": 0.35}, 4, "flicker"),
    "ink_wash": ("pluck", {"bright": 0.35, "ring": 1.4}, 0, "pluck", {"bright": 0.3, "ring": 1.0},
                 2, None),
    "graphite": ("pluck", {"bright": 0.55, "ring": 0.5}, 0, "pluck", {"bright": 0.5, "ring": 0.35},
                 2, "scratch"),
    "watercolor": ("flute", {}, 0, None, {}, 0, "wash"),
    "coloured_pencil": ("marimba", {}, 0, "marimba", {"vel": 0.4}, 2, "scratch"),
    "blueprint": ("bell", {"ratio": 1.0, "index": 0.7, "ring": 0.9}, 0, "bleep", {}, 2, None),
    "charcoal": ("epiano", {"vel": 0.7}, -12, None, {}, 0, "scratch"),
}

# 02's six tables, one every 2.4 s: source, phosphor tint, CGA on black,
# amber blocks, mono, then source with the cells settling.
TYPE_TABLES = [
    {"duty": 0.5}, {"duty": 0.25, "vibrato": 0.6}, {"duty": 0.125}, {"duty": 0.5, "oct": -12},
    {"duty": 0.5, "bright": 0.3}, {"duty": 0.25},
]

HANDS = ["epiano", "pulse", "pluck", "flute", "bell", "marimba"]

# Mix per family, in dB on each bus, set by the event script at each shot.
MIX = {
    "plain": {"foundation": -4, "harmony": -2, "texture": 0},
    "type": {"harmony": -5, "world_voice": -2, "home_theme": -1, "texture": -3},
    "edges": {"harmony": -5, "home_theme": -2, "texture": -3},
    "entities": {"foundation": -1, "harmony": -3, "world_voice": -3},
    "mask": {"foundation": -3, "harmony": -2, "world_voice": -2, "home_theme": -3},
    "story": {"foundation": -2, "harmony": -3, "world_voice": -2},
    "reveal": {"foundation": -1, "harmony": -3, "world_voice": -2},
    "pencil": {"foundation": -6, "harmony": -4},
    "watercolor": {"foundation": -3, "harmony": 1, "texture": -2},
    "looks": {"harmony": -4, "world_voice": -1},
    "user": {"foundation": -2, "harmony": -3, "world_voice": -1},
    "menu": {"foundation": -4, "harmony": -2, "world_voice": -6, "home_theme": -2},
    "hands": {},
    "coda": {},
}


class Arranger:
    def __init__(self, cut: Cut):
        self.cut = cut
        self.seed = zlib.crc32(cut.name.encode())
        self.rng = np.random.default_rng(self.seed)
        self.stems = {b: Stem(cut.seconds) for b in BUSES}
        bars = int(cut.bars)
        self.coda_from = (bars - 2) * BAR  # the last two bars come home
        self.cadence = (bars - 3) * BAR

    # helpers
    def play(self, bus: str, voice: str, beat: float, note: float, beats: float, pan=0.0,
             gain=1.0, send=0.25, **kw):
        fn = synth.VOICES[voice]
        sig = fn(midi_hz(note), beats * BEAT_S, **kw)
        self.stems[bus].add(beat * BEAT_S, sig, pan=pan, gain=gain, send=send)

    def melody_events(self):
        """(beat, note, beats) for the whole cut, with the cadence and coda."""
        out = []
        for bar in range(int(self.cut.bars)):
            b0 = bar * BAR
            if b0 >= self.coda_from:
                if b0 == self.coda_from:
                    out.append((b0, 74, 8.0))
                continue
            if b0 == self.cadence:
                out += [(b0, 76, 1.5), (b0 + 1.5, 73, .5), (b0 + 2, 76, 1), (b0 + 3, 73, 1)]
                continue
            name, pb = _phrase(bar)
            for (t, m, d) in MELODY[name]:
                if pb * BAR <= t < (pb + 1) * BAR:
                    out.append((b0 + t - pb * BAR, m, d))
        return out

    def chord_events(self):
        out = []
        for bar in range(int(self.cut.bars)):
            b0 = bar * BAR
            if b0 >= self.coda_from:
                if b0 == self.coda_from:
                    out.append((b0, "Dmaj9", 8.0))
                continue
            if b0 == self.cadence:
                out += [(b0, "Em9", 2), (b0 + 2, "A7", 2)]
                continue
            name, pb = _phrase(bar)
            out += [(b0 + t, c, d) for (t, c, d) in CHORDS[name][pb]]
        return out

    def chord_at(self, beat: float) -> str:
        cur = "Dmaj7"
        for (t, c, _) in self.chord_events():
            if t <= beat + 1e-9:
                cur = c
        return cur

    # the parts
    def melody(self):
        hands_i = 0
        last_bar = -1
        for (b, m, d) in self.melody_events():
            shot = self.cut.shot_at(b)
            fam, tt = shot.family, shot.take_time(b)
            dur = d
            if b >= self.coda_from:
                fam = "coda"  # home, whatever shot the last two bars sit in
            if fam == "plain" or fam == "entities" or fam == "story" or fam == "coda":
                self.play("home_theme", "epiano", b, m, dur, send=0.3, vel=0.75)
            elif fam == "type":
                k = min(int(tt // 2.4), len(TYPE_TABLES) - 1)
                p = dict(TYPE_TABLES[k])
                oct_ = p.pop("oct", 0)
                self.play("home_theme", "pulse", b, m + oct_, dur * 0.9, send=0.2, vel=0.8, **p)
            elif fam == "edges":
                self.play("home_theme", "pulse", b, m, dur * 0.9, send=0.22, vel=0.75, duty=0.25)
            elif fam == "mask" or fam == "menu":
                self.play("home_theme", "music_box", b, m + 12, dur, send=0.35, vel=0.6)
            elif fam == "reveal":
                bl = _reveal_blend(tt)
                self.play("home_theme", "epiano", b, m, dur, send=0.3, vel=0.75 * (1 - 0.5 * bl))
                if bl > 0.02:
                    self.play("home_theme", "pulse", b, m + 12, dur * 0.85, send=0.2,
                              vel=0.55 * bl, duty=0.25)
            elif fam == "pencil":
                self.play("home_theme", "pluck", b, m, dur, send=0.15, vel=0.9, bright=0.5,
                          ring=0.7)
            elif fam == "watercolor":
                self.play("home_theme", "flute", b, m, dur, send=0.55, vel=0.8)
            elif fam == "looks":
                voice, args, oct_, *_ = LOOKS.get(shot.take, LOOKS["charcoal"])
                kw = {"vel": 0.8, **args}
                self.play("home_theme", voice, b, m + oct_, dur, send=0.3, **kw)
            elif fam == "user":
                self.play("home_theme", "bell", b, m, dur, send=0.4, vel=0.8, ratio=3.5,
                          index=1.6, ring=1.2)
            elif fam == "hands":
                bar = int(b // BAR)
                if bar != last_bar:
                    hands_i, last_bar = hands_i + 1, bar
                voice = HANDS[hands_i % len(HANDS)]
                kw = {"duty": 0.25} if voice == "pulse" else {}
                if voice == "bell":
                    kw = {"ratio": 3.5, "index": 1.4}
                pan = (-0.35, 0.35)[hands_i % 2]
                self.play("home_theme", voice, b, m, dur, pan=pan, send=0.3, vel=0.8, **kw)
                self.play("home_theme", "epiano", b, m, dur, send=0.3, vel=0.32)

    def harmony(self):
        for (b, c, d) in self.chord_events():
            shot = self.cut.shot_at(b)
            fam = shot.family
            bright = {"type": 0.6, "edges": 0.6, "watercolor": 0.3, "pencil": 0.35,
                      "user": 0.7, "looks": 0.55, "hands": 0.5, "coda": 0.4}.get(fam, 0.45)
            send = 0.7 if fam in ("watercolor", "coda") else 0.45
            sig = synth.pad([midi_hz(n) for n in V[c]], d * BEAT_S, vel=0.95, bright=bright,
                            seed=self.seed + int(b * 4))
            self.stems["harmony"].add(b * BEAT_S, sig, send=send)

    def foundation(self):
        for (b, c, d) in self.chord_events():
            root = ROOT[c]
            steps = []
            shot = self.cut.shot_at(b)
            fam = shot.family
            if fam == "coda" or b >= self.coda_from:
                steps = [(0, d)]
            elif fam == "plain":
                if b - shot.start >= BAR - 1e-9 or shot.start > 0:
                    steps = [(0, d)]
            elif fam in ("watercolor", "menu", "mask"):
                steps = [(0, d)]
            elif fam == "looks":
                steps = [(x * 0.5, 0.45) for x in range(int(d * 2))]
            else:
                steps = [(x, 2 if d - x >= 2 else d - x) for x in range(0, int(d), 2)]
            for (o, dd) in steps:
                bb = b + o
                s2 = self.cut.shot_at(bb)
                if s2.family != fam:
                    continue
                octave = 12 if (fam == "looks" and int(o * 2) % 2) else 0
                self.play("foundation", "bass", bb, root + octave, dd * 0.95, send=0.05,
                          vel=0.8, gain=0.5)
        # the heartbeat
        for beat in np.arange(0, self.coda_from, 2.0):
            fam = self.cut.shot_at(beat).family
            if fam in ("type", "edges", "entities", "reveal", "looks", "hands", "user"):
                self.stems["foundation"].add(beat * BEAT_S, synth.kick(vel=0.4), send=0.05)

    def arps(self):
        step_cache = {}
        beat = 0.0
        while beat < self.coda_from:
            shot = self.cut.shot_at(beat)
            fam, tt = shot.family, shot.take_time(beat)
            spec = self._arp_spec(shot, tt)
            if spec is None:
                beat += 0.25
                continue
            voice, per_beat, oct_, kw, pan_mode = spec
            step = 1.0 / per_beat
            if abs((beat / step) - round(beat / step)) > 1e-6:
                beat += 0.25
                continue
            notes = sorted(V[self.chord_at(beat)])
            ladder = [n + oct_ for n in notes] + [n + 12 + oct_ for n in notes]
            ladder = ladder + ladder[-2:0:-1]
            i = int(round(beat / step))
            n = ladder[i % len(ladder)]
            if pan_mode == "alt":
                pan = -0.5 if i % 2 else 0.5
            elif pan_mode == "random":
                pan = float(self.rng.uniform(-0.8, 0.8))
            else:
                pan = 0.0
            prob = kw.pop("_prob", 1.0)
            if prob >= 1.0 or self.rng.random() < prob:
                self.play("world_voice", voice, beat, n, step * 0.9, pan=pan, send=0.3, **kw)
            beat += step

    def _arp_spec(self, shot: Shot, tt: float):
        fam = shot.family
        if fam == "type":
            k = min(int(tt // 2.4), len(TYPE_TABLES) - 1)
            p = dict(TYPE_TABLES[k])
            oct_ = p.pop("oct", 0)
            per = 2 if oct_ else 4
            return ("pulse", per, 12 + oct_, {"vel": 0.42, **p}, "alt")
        if fam == "edges":
            if tt < 2.5:
                return ("pulse", 4, 12, {"vel": 0.38, "duty": 0.25}, "alt")
            return ("pluck", 4, 12, {"vel": 0.55, "bright": 0.8, "ring": 0.35}, "alt")
        if fam == "entities":
            return ("pulse", 2, 12, {"vel": 0.4, "duty": 0.125}, "alt")
        if fam == "mask":
            if 3.4 <= tt < 7.1:
                return ("bleep", 2, 12, {"vel": 0.6}, "random")
            return None
        if fam == "story":
            if 1.2 <= tt < 3.2:
                return ("pulse", 2, 12, {"vel": 0.4, "duty": 0.25}, "alt")
            if 3.2 <= tt < 5.2:
                return ("pulse", 2, 0, {"vel": 0.45, "duty": 0.5, "vibrato": 0.4}, "alt")
            if 5.2 <= tt < 7.4:
                return ("pluck", 2, 12, {"vel": 0.55, "bright": 0.5, "ring": 0.4}, "alt")
            return None
        if fam == "reveal":
            return ("pulse", 4, 12, {"vel": 0.42, "duty": 0.125, "_prob": _reveal_blend(tt)},
                    "random")
        if fam == "pencil":
            return ("pluck", 2, 12, {"vel": 0.6, "bright": 0.45, "ring": 0.4}, "alt")
        if fam == "looks":
            _, _, _, voice, args, per, _ = LOOKS.get(shot.take, LOOKS["charcoal"])
            if voice is None:
                return None
            return (voice, per, 12, {"vel": 0.5, **args}, "alt")
        if fam == "user":
            return ("bell", 2, 12, {"vel": 0.45, "ratio": 3.5, "index": 1.0, "ring": 0.8}, "alt")
        if fam == "hands":
            return ("pluck", 2, 12, {"vel": 0.5, "bright": 0.6, "ring": 0.5}, "alt")
        return None

    def motif(self):
        bars = int(self.cut.bars)
        for bar in range(bars):
            b0 = bar * BAR
            shot = self.cut.shot_at(b0)
            fam = shot.family
            local = int(round((b0 - shot.start) / BAR))
            call = False
            if fam == "plain" and local == 1:
                call = True
            if fam == "story" and local == 0:
                call = True
            if fam == "hands" and local % 2 == 1 and b0 < self.cadence:
                call = True
            if fam == "watercolor" and local == 1:
                call = True
            if call:
                for (t, m, d) in MOTIF:
                    self.play("child_motif", "bell", b0 + t, m, d, pan=0.4, send=0.5, vel=0.55,
                              ratio=3.0, index=1.0, ring=1.4)
        for (t, m, d) in MOTIF_HOME:
            self.play("child_motif", "bell", self.coda_from + t, m, d, pan=0.3, send=0.6,
                      vel=0.5, ratio=3.0, index=0.9, ring=2.0)

    def texture(self):
        stem = self.stems["texture"]
        for i, shot in enumerate(self.cut.shots):
            fam = shot.family
            t0 = shot.start * BEAT_S
            secs = shot.length * BEAT_S
            n = int(secs * SR)
            seed = self.seed + 97 * i
            if fam in ("plain", "story", "coda", "hands", "user", "entities"):
                dens = {"entities": 0.5, "user": 0.6}.get(fam, 1.0)
                stem.add_bed(t0, synth.crickets(n, seed, density=dens), gain=2.8, fade=0.15)
            if fam in ("plain", "coda"):
                stem.add_bed(t0, synth.filtered_noise(n, seed + 3, None, 500.0) * 0.035,
                             send=0.0, fade=0.4)
            bed = None
            if fam == "looks":
                bed = LOOKS.get(shot.take, LOOKS["charcoal"])[6]
            if fam in ("type", "edges", "reveal", "entities", "hands") or bed == "flicker":
                self._flicker(shot, scale=0.4 if fam in ("hands", "entities") else 1.0)
            if fam in ("pencil", "hands") or bed == "scratch":
                sc = synth.scratch(n, seed + 5)
                if fam == "pencil":
                    tt = shot.start_s + np.arange(n) / SR
                    sc *= np.where(tt >= 5.0, 1.0, 0.55)[:, None]
                stem.add_bed(t0, sc, gain=0.6 if fam == "hands" else 1.8, fade=0.05)
            if fam == "watercolor" or bed == "wash":
                wash = synth.filtered_noise(n, seed + 7, 180.0, 1100.0)
                tt = shot.start_s + np.arange(n) / SR
                swell = 0.6 + 0.4 * np.sin(2 * np.pi * tt / 4.8) ** 2
                if fam == "watercolor":
                    swell *= np.clip(1.0 - (tt - 5.0) / 1.5, 0.35, 1.0)
                stem.add_bed(t0, wash * swell[:, None] * 0.11, send=0.4, fade=0.3)
            if fam in ("mask", "menu"):
                for c in _clicks(shot.beat):
                    at = (c - shot.start_s) / BEAT_S + shot.start
                    if shot.start <= at < shot.start + shot.length:
                        stem.add(at * BEAT_S, synth.click(seed=int(c * 100)), pan=0.55, send=0.1)

    def _flicker(self, shot: Shot, scale: float):
        """Glyph flicker: a few soft ticks a beat, as cells change."""
        beat = np.ceil(shot.start * 4) / 4
        while beat < min(shot.start + shot.length, self.coda_from):
            prob = 0.28 * scale
            if shot.family == "reveal":
                prob *= _reveal_blend(shot.take_time(beat)) + 0.1
            if self.rng.random() < prob:
                f = float(self.rng.choice([3200.0, 3800.0, 4400.0, 5200.0]))
                self.stems["texture"].add(beat * BEAT_S, synth.click(f, vel=0.6,
                                          seed=int(beat * 16)),
                                          pan=float(self.rng.uniform(-0.7, 0.7)), send=0.15)
            beat += 0.25

    def build(self) -> dict[str, np.ndarray]:
        self.melody()
        self.harmony()
        self.foundation()
        self.arps()
        self.motif()
        self.texture()
        ir = synth.reverb_ir()
        return {b: s.render(ir) for b, s in self.stems.items()}


# --- the suite and its mix automation ----------------------------------------


def suite_toml(cut: Cut, stems_dir: str) -> str:
    bar_samples = int(BAR * BEAT_S * SR)
    lines = [
        f"# *Many Hands*, the showcase film's music: the {cut.name} cut (#438).",
        "# Written by tools/showcase/music.py from tools/showcase/cuts/"
        f"{cut.name}.toml; edit the cut",
        "# list or score.py and rebuild, never this file. Every sound is original:",
        "# synthesized by tools/showcase/synth.py, no samples.",
        "schema_version = 0",
        "",
        "[suite]",
        f'id = "many-hands-{cut.name}"',
        f'title = "Many Hands ({cut.name})"',
        "",
        "[audio]",
        f"sample_rate = {SR}",
        "",
        "[[tempo]]",
        "sample = 0",
        "bpm = 100.0",
        "time_signature = [4, 4]",
        "",
    ]
    names, seen = [], set()
    for i, s in enumerate(cut.shots):
        if abs(s.start / BAR - round(s.start / BAR)) > 1e-9 or s.beat in seen and s.beat == "10_looks":
            continue
        seen.add(s.beat)
        name = f"{i + 1:02d}_{s.beat}" if s.beat != "tail" else "tail"
        names.append(name)
        lines += ["[[sections]]", f'name = "{name}"',
                  f"start_sample = {int(round(s.start / BAR)) * bar_samples}",
                  "pulse_window_ms = 100", ""]
    lines += ["[reintegration]", f're_entry_sections = ["{names[0]}"]',
              'lead_bus = "home_theme"', "reassembly_bars = 2", ""]
    for b in BUSES:
        lines += [f"[buses.{b}]", f'file = "{stems_dir}/{b}.wav"', ""]
    return "\n".join(lines)


def events_toml(cut: Cut) -> str:
    ev = []

    def add(beat, action, bus=None, **kw):
        ev.append((beat, action, bus, kw))

    for s in cut.shots:
        add(s.start, "marker", label=f"{s.beat}/{s.take}")
        mix = MIX.get(s.family, {})
        for b in BUSES:
            add(s.start, "set_gain", b, db=float(mix.get(b, 0)), ramp_ms=150.0)
        # cues inside the shot, in take seconds
        def at(take_s):
            beat = s.start + (take_s - s.start_s) / BEAT_S
            return beat if s.start <= beat < s.start + s.length else None
        if s.family == "watercolor":
            b = at(5.0)
            if b is not None:
                add(b, "set_lpf", "harmony", hz=1400.0, ramp_ms=1800.0)
        if s.family == "menu":
            for (ts, hz, ramp) in ((2.8, 650.0, 1100.0), (3.9, 20000.0, 1300.0)):
                b = at(ts)
                if b is not None:
                    add(b, "set_lpf", "harmony", hz=hz, ramp_ms=ramp)
                    add(b, "set_lpf", "home_theme", hz=max(hz * 2, 1300.0), ramp_ms=ramp)
        # every shot after starts open
        for b in ("harmony", "home_theme"):
            if s.family in ("watercolor", "menu"):
                add(s.start + s.length, "set_lpf", b, hz=20000.0, ramp_ms=300.0)
    # the last bar fades as the chord rings out
    last = cut.beats - BAR
    for b in BUSES:
        fade = -18.0 if b == "texture" else -40.0
        add(last, "set_gain", b, db=fade, ramp_ms=BAR * BEAT_S * 1000.0)

    ev.sort(key=lambda e: (e[0], e[1] != "marker"))
    lines = [f"# Mix automation for the {cut.name} cut (#438), written by tools/showcase/music.py:",
             "# a marker at every shot, each shot's bus levels, the watercolor's lift and",
             "# the menu drag as low-passes, and the last bar's fade. Times in beats",
             "# from the start (100 bpm: a beat is 0.6 s).",
             "schema_version = 0", ""]
    for (beat, action, bus, kw) in ev:
        lines.append("[[events]]")
        lines.append(f'at = "beat:{beat:g}"')
        if bus:
            lines.append(f'bus = "{bus}"')
        lines.append(f'action = "{action}"')
        for k, v in kw.items():
            lines.append(f'{k} = "{v}"' if isinstance(v, str) else f"{k} = {v}")
        lines.append("")
    return "\n".join(lines)
