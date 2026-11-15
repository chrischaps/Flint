"""The two stems of examples/music_clock.scene.toml (#745).

Eight bars at 96 bpm in 4/4, 20 seconds, at 22050 Hz, mono:

- `pad`:  a soft chord a bar (Dmaj7, Bm7, Gmaj7, A6, twice), each swelling
          in and letting go before the next; the second four bars add the
          octave above.
- `bell`: a struck bell on every beat, the chord's root an octave up on the
          one and its third and fifth after; the second four bars add a
          quiet bell on each off-beat.

The suite names the first four bars `dawn` and the last four `rise`, and its
event script opens the pad's low-pass on the way into `rise`. Deterministic.
Writes examples/music/stems/{pad,bell}.flac (through ffmpeg, which must be on
PATH: FLAC keeps the stems small and lossless, so they stay sample-locked).

    python examples/tools/music_clock.py
"""

import math
import os
import struct
import subprocess
import tempfile
import wave

RATE = 22_050
BPM = 96.0
BEAT = 60.0 / BPM
BARS = 8
LENGTH = int(round(BARS * 4 * BEAT * RATE))

# A bar each: midi notes, root first
CHORDS = [
    [50, 54, 57, 61],  # Dmaj7
    [47, 50, 54, 57],  # Bm7
    [43, 47, 50, 54],  # Gmaj7
    [45, 49, 52, 54],  # A6
] * 2


def hz(midi):
    return 440.0 * 2 ** ((midi - 69) / 12)


def pad():
    out = [0.0] * LENGTH
    bar_len = 4 * BEAT
    for bar, chord in enumerate(CHORDS):
        start = int(bar * bar_len * RATE)
        n = int(bar_len * RATE)
        notes = chord + ([m + 12 for m in chord] if bar >= 4 else [])
        for k, midi in enumerate(notes):
            f = hz(midi)
            level = 0.05 if k < 4 else 0.03
            for detune in (-0.0015, 0.0015):
                w = 2 * math.pi * f * (1 + detune) / RATE
                phase = k * 0.7
                for i in range(n):
                    t = i / n
                    # swell in over a third of the bar, let go over the last fifth
                    env = min(1.0, t * 3.0) * min(1.0, (1.0 - t) * 5.0)
                    s = math.sin(w * i + phase) + 0.18 * math.sin(2 * w * i + phase)
                    out[start + i] += level * env * s
    return out


def bell(out, at, freq, level):
    start = int(at * RATE)
    n = min(int(1.6 * RATE), LENGTH - start)
    for ratio, gain, decay in ((1.0, 1.0, 0.9), (2.76, 0.45, 0.35), (5.4, 0.22, 0.15)):
        w = 2 * math.pi * freq * ratio / RATE
        for i in range(n):
            out[start + i] += level * gain * math.exp(-i / (decay * RATE)) * math.sin(w * i)


def bells():
    out = [0.0] * LENGTH
    for bar, chord in enumerate(CHORDS):
        root, third, fifth = chord[0] + 12, chord[1] + 12, chord[2] + 12
        for beat, midi in enumerate((root + 12, fifth, third, fifth)):
            at = (bar * 4 + beat) * BEAT
            bell(out, at, hz(midi), 0.32 if beat == 0 else 0.2)
            if bar >= 4:
                bell(out, at + BEAT / 2, hz(fifth + 12), 0.07)
    return out


def write_flac(path, samples):
    peak = max(abs(s) for s in samples) or 1.0
    scale = 0.7 / peak
    with tempfile.TemporaryDirectory() as tmp:
        wav = os.path.join(tmp, "stem.wav")
        with wave.open(wav, "wb") as w:
            w.setnchannels(1)
            w.setsampwidth(2)
            w.setframerate(RATE)
            w.writeframes(
                b"".join(struct.pack("<h", int(round(s * scale * 32767))) for s in samples)
            )
        subprocess.run(
            ["ffmpeg", "-y", "-loglevel", "error", "-i", wav, "-c:a", "flac",
             "-compression_level", "12", "-map_metadata", "-1", path],
            check=True,
        )


def main():
    here = os.path.dirname(os.path.abspath(__file__))
    stems = os.path.join(here, "..", "music", "stems")
    os.makedirs(stems, exist_ok=True)
    write_flac(os.path.join(stems, "pad.flac"), pad())
    write_flac(os.path.join(stems, "bell.flac"), bells())
    print(f"wrote {stems}: {BARS} bars at {BPM:g} bpm, {LENGTH} frames at {RATE} Hz")


if __name__ == "__main__":
    main()
