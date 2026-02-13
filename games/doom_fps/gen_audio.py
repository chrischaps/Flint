"""Generate ambient audio loops for E1M1 Hangar level (no external deps)."""
import wave
import struct
import math
import random
import os

OUT = os.path.join(os.path.dirname(__file__), "audio")
SAMPLE_RATE = 22050
CHANNELS = 1
SAMPLE_WIDTH = 2  # 16-bit


def write_wav(filename, samples, sample_rate=SAMPLE_RATE):
    """Write a list of float samples (-1..1) to a WAV file."""
    path = os.path.join(OUT, filename)
    with wave.open(path, 'w') as wf:
        wf.setnchannels(CHANNELS)
        wf.setsampwidth(SAMPLE_WIDTH)
        wf.setframerate(sample_rate)
        for s in samples:
            clamped = max(-1.0, min(1.0, s))
            wf.writeframes(struct.pack('<h', int(clamped * 32767)))
    print(f"  {filename} ({len(samples)} samples, {len(samples)/sample_rate:.1f}s)")


def gen_nukage_sizzle():
    """Toxic bubbling/sizzling ambient loop (~3 seconds).

    Layers:
    - Brown noise base (low rumble)
    - Periodic bubble pops (random timing)
    - High-frequency fizz overlay
    """
    duration = 3.0
    n_samples = int(SAMPLE_RATE * duration)
    samples = [0.0] * n_samples
    random.seed(1234)

    # Layer 1: Brown noise (integrated white noise)
    brown = 0.0
    for i in range(n_samples):
        brown += random.gauss(0, 0.02)
        brown *= 0.998  # slight decay to prevent drift
        samples[i] += brown * 0.4

    # Layer 2: Bubble pops (short sine bursts at random intervals)
    t = 0.0
    while t < duration:
        t += random.uniform(0.08, 0.3)
        pop_start = int(t * SAMPLE_RATE)
        pop_len = int(random.uniform(0.01, 0.04) * SAMPLE_RATE)
        pop_freq = random.uniform(300, 800)
        pop_vol = random.uniform(0.05, 0.15)
        for j in range(pop_len):
            idx = pop_start + j
            if idx < n_samples:
                env = math.sin(math.pi * j / pop_len)  # smooth envelope
                samples[idx] += math.sin(2 * math.pi * pop_freq * j / SAMPLE_RATE) * env * pop_vol

    # Layer 3: High-frequency fizz
    for i in range(n_samples):
        fizz = random.gauss(0, 0.03)
        # Bandpass-ish: modulate with high freq
        fizz *= math.sin(2 * math.pi * 4000 * i / SAMPLE_RATE) * 0.5
        samples[i] += fizz

    # Crossfade ends for seamless loop (50ms)
    fade_len = int(0.05 * SAMPLE_RATE)
    for i in range(fade_len):
        t_fade = i / fade_len
        samples[i] = samples[i] * t_fade + samples[n_samples - fade_len + i] * (1 - t_fade)
    # Trim the crossfade tail
    samples = samples[:n_samples - fade_len]

    # Normalize
    peak = max(abs(s) for s in samples)
    if peak > 0:
        samples = [s / peak * 0.7 for s in samples]

    write_wav("nukage_sizzle.wav", samples)


def gen_computer_hum():
    """Low electronic hum ambient loop (~2 seconds).

    Layers:
    - 60 Hz fundamental (mains hum)
    - Harmonics at 120 Hz and 180 Hz
    - Subtle warble/modulation
    """
    duration = 2.0
    n_samples = int(SAMPLE_RATE * duration)
    samples = [0.0] * n_samples

    for i in range(n_samples):
        t = i / SAMPLE_RATE
        # Fundamental 60 Hz
        s = math.sin(2 * math.pi * 60 * t) * 0.5
        # 2nd harmonic 120 Hz
        s += math.sin(2 * math.pi * 120 * t) * 0.25
        # 3rd harmonic 180 Hz
        s += math.sin(2 * math.pi * 180 * t) * 0.12
        # Subtle warble (slow LFO on amplitude)
        warble = 1.0 + 0.08 * math.sin(2 * math.pi * 0.5 * t)
        s *= warble
        # Tiny noise floor
        s += random.gauss(0, 0.005)
        samples[i] = s

    # Normalize
    peak = max(abs(s) for s in samples)
    if peak > 0:
        samples = [s / peak * 0.6 for s in samples]

    write_wav("computer_hum.wav", samples)


if __name__ == "__main__":
    print("Generating ambient audio loops...")
    gen_nukage_sizzle()
    gen_computer_hum()
    print(f"Done! Audio files in {OUT}/")
