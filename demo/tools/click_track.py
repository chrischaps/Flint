"""The click track for demo/audio_clock.scene.toml (#658).

A two-second loop at 120 bpm, four beats of 24000 frames at 48 kHz: a high
click on the one, lower ones on two to four. Each click is a short sine
burst with a fast decay, so where it lands is easy to hear against a pad
lighting up. Deterministic; writes demo/audio/click_120.wav.

    python demo/tools/click_track.py
"""

import math
import os
import struct
import wave

RATE = 48_000
BEAT = RATE // 2  # 120 bpm
BEATS = 4


def click(freq, length=0.06, decay=0.012, level=0.6):
    n = int(length * RATE)
    return [
        level * math.exp(-i / (decay * RATE)) * math.sin(2 * math.pi * freq * i / RATE)
        for i in range(n)
    ]


def main():
    samples = [0.0] * (BEAT * BEATS)
    for beat in range(BEATS):
        for i, s in enumerate(click(1760.0 if beat == 0 else 1175.0)):
            samples[beat * BEAT + i] += s
    out = os.path.join(os.path.dirname(__file__), "..", "audio", "click_120.wav")
    with wave.open(out, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(
            b"".join(struct.pack("<h", int(max(-1.0, min(1.0, s)) * 32767)) for s in samples)
        )
    print(f"wrote {os.path.normpath(out)}: {len(samples)} frames, {len(samples) / RATE} s")


if __name__ == "__main__":
    main()
