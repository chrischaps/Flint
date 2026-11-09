"""The showcase film's instruments (#438): a small additive/FM synthesizer in
numpy that writes the stems `flint render-suite` mixes.

Every sound in the film is made here, from sines, noise and arithmetic: no
samples, no presets, nothing from anywhere else, so YouTube's Content ID
and Instagram's audio match have nothing to find. All randomness comes from
seeded generators, so two runs write byte-identical stems.

A voice is a function returning a stereo (n, 2) float array that starts at
its note's onset and runs through its release. `Stem.add` lays it in at a
time, pans it and splits it between a dry bus and a reverb send.
"""

from __future__ import annotations

import numpy as np
from scipy import signal

SR = 48_000


def midi_hz(m: float) -> float:
    return 440.0 * 2.0 ** ((m - 69.0) / 12.0)


def _t(n: int) -> np.ndarray:
    return np.arange(n, dtype=np.float64) / SR


def _env(n: int, dur: float, attack: float, release: float, decay: float | None = None,
         sustain: float = 1.0) -> np.ndarray:
    """Attack, optional exponential decay towards `sustain`, release after `dur`."""
    t = _t(n)
    a = np.clip(t / max(attack, 1e-4), 0.0, 1.0)
    a = a * a * (3.0 - 2.0 * a)  # smooth start: no click
    if decay is not None:
        a = a * (sustain + (1.0 - sustain) * np.exp(-np.maximum(t - attack, 0.0) / decay))
    r = np.where(t > dur, np.exp(-(t - dur) / max(release, 1e-4) * 4.0), 1.0)
    return a * r


def _length(dur: float, tail: float) -> int:
    return int((dur + tail) * SR)


def _stereo(mono: np.ndarray, width: float = 0.0, seed: int = 0) -> np.ndarray:
    """Mono to stereo; `width` adds a few-sample offset to one side."""
    if width <= 0.0:
        return np.stack([mono, mono], axis=1)
    d = int(width * 0.0006 * SR)
    r = np.concatenate([np.zeros(d), mono[: len(mono) - d]]) if d else mono
    return np.stack([mono, r], axis=1)


def _partials(f: float, n: int, amps, ratios, phases=None, decays=None) -> np.ndarray:
    t = _t(n)
    out = np.zeros(n)
    for i, (a, r) in enumerate(zip(amps, ratios)):
        fr = f * r
        if fr >= SR * 0.45 or a == 0.0:
            continue
        ph = 0.0 if phases is None else phases[i]
        s = a * np.sin(2.0 * np.pi * fr * t + ph)
        if decays is not None:
            s *= np.exp(-t / decays[i])
        out += s
    return out


# --- voices -------------------------------------------------------------------


def epiano(f: float, dur: float, vel: float = 0.8, **_) -> np.ndarray:
    """A two-operator electric piano: a tine's bark that mellows as it rings."""
    n = _length(dur, 1.6)
    t = _t(n)
    index = (0.6 + 1.6 * vel) * np.exp(-t / 0.45)
    out = np.zeros((n, 2))
    for side, cents in ((0, -2.0), (1, 2.0)):
        fc = f * 2.0 ** (cents / 1200.0)
        car = np.sin(2 * np.pi * fc * t + index * np.sin(2 * np.pi * fc * t))
        bark = 0.18 * vel * np.sin(2 * np.pi * fc * 7.0 * t) * np.exp(-t / 0.03)
        out[:, side] = car + bark
    env = _env(n, dur, 0.004, 0.35, decay=1.8, sustain=0.0)
    return out * env[:, None] * vel * 0.5


def pulse(f: float, dur: float, vel: float = 0.8, duty: float = 0.5, bright: float = 1.0,
          vibrato: float = 0.0, **_) -> np.ndarray:
    """A band-limited pulse wave: the terminal's voice for the type beats."""
    n = _length(dur, 0.08)
    t = _t(n)
    top = int(min(40, (9000.0 * bright) // f))
    k = np.arange(1, max(top, 1) + 1)
    amps = np.sin(np.pi * k * duty) / k
    if vibrato > 0.0:
        ph = 2 * np.pi * f * t + vibrato * np.sin(2 * np.pi * 5.5 * t) * np.clip(t / 0.4, 0, 1)
    else:
        ph = 2 * np.pi * f * t
    mono = np.zeros(n)
    for a, kk in zip(amps, k):
        mono += a * np.sin(kk * ph)
    mono *= _env(n, dur, 0.003, 0.03, decay=0.35, sustain=0.65) * vel * 0.32
    return _stereo(mono)


def pluck(f: float, dur: float, vel: float = 0.8, bright: float = 0.7, ring: float = 0.9,
          **_) -> np.ndarray:
    """A plucked string: high partials die first. Edges, pencil, ink."""
    n = _length(min(dur, ring), ring * 1.4)
    k = np.arange(1, 24)
    amps = (bright ** (k - 1)) / k * (1.0 + 0.4 * np.sin(k * 1.7))
    decays = ring / (1.0 + 0.55 * (k - 1))
    mono = _partials(f, n, amps, k * (1.0 + 0.0004 * k * k), decays=decays)
    t = _t(n)
    mono += 0.25 * vel * np.random.default_rng(int(f * 10)).standard_normal(n) * np.exp(-t / 0.004)
    mono *= _env(n, n / SR, 0.0015, 0.05) * vel * 0.35
    return _stereo(mono, width=1.0)


def flute(f: float, dur: float, vel: float = 0.8, **_) -> np.ndarray:
    """A breathy wooden flute for the watercolor: a sine, its octave, and air."""
    n = _length(dur, 0.35)
    t = _t(n)
    vib = 0.004 * np.sin(2 * np.pi * 5.1 * t) * np.clip((t - 0.25) / 0.5, 0, 1)
    ph = 2 * np.pi * f * t + 2 * np.pi * f * np.cumsum(vib) / SR
    mono = np.sin(ph) + 0.22 * np.sin(2 * ph) + 0.06 * np.sin(3 * ph)
    rng = np.random.default_rng(int(f * 7))
    air = rng.standard_normal(n)
    sos = signal.butter(2, [min(f * 1.5, 9000), min(f * 4.0, 15000)], "bandpass", fs=SR,
                        output="sos")
    mono += 0.12 * signal.sosfilt(sos, air)
    mono *= _env(n, dur, 0.09, 0.15) * vel * 0.42
    return _stereo(mono, width=0.6)


def bell(f: float, dur: float, vel: float = 0.8, ratio: float = 3.5, index: float = 2.2,
         ring: float = 1.6, **_) -> np.ndarray:
    """FM glass: the holograms of the user styles, the figure's little call."""
    n = _length(0.0, ring * 2.2)
    t = _t(n)
    i = index * vel * np.exp(-t / (ring * 0.4))
    mono = np.sin(2 * np.pi * f * t + i * np.sin(2 * np.pi * f * ratio * t))
    mono += 0.3 * np.sin(2 * np.pi * f * 2.001 * t) * np.exp(-t / (ring * 0.5))
    mono *= _env(n, n / SR, 0.002, 0.1, decay=ring, sustain=0.0) * vel * 0.33
    return _stereo(mono, width=0.8)


def music_box(f: float, dur: float, vel: float = 0.8, **_) -> np.ndarray:
    n = _length(0.0, 1.8)
    mono = _partials(f, n, [1.0, 0.25, 0.08], [1.0, 3.01, 5.43], decays=[0.9, 0.25, 0.08])
    mono *= _env(n, n / SR, 0.001, 0.1) * vel * 0.3
    return _stereo(mono)


def marimba(f: float, dur: float, vel: float = 0.8, **_) -> np.ndarray:
    n = _length(0.0, 1.0)
    mono = _partials(f, n, [1.0, 0.35, 0.08], [1.0, 3.93, 9.2], decays=[0.45, 0.07, 0.02])
    mono *= _env(n, n / SR, 0.0015, 0.05) * vel * 0.4
    return _stereo(mono)


def bleep(f: float, dur: float, vel: float = 0.8, **_) -> np.ndarray:
    """A clean sine blip: one hue per style id."""
    n = _length(min(dur, 0.12), 0.06)
    t = _t(n)
    mono = np.sin(2 * np.pi * f * t) * _env(n, min(dur, 0.12), 0.002, 0.04) * vel * 0.25
    return _stereo(mono)


_TABLE = 4096


def pad(freqs, dur: float, vel: float = 0.7, bright: float = 0.5, seed: int = 0,
        **_) -> np.ndarray:
    """A chord of soft detuned saws, the evening light under everything."""
    n = _length(dur, 1.4)
    t = _t(n)
    rng = np.random.default_rng(seed)
    cutoff = 400.0 + 3600.0 * bright
    out = np.zeros((n, 2))
    for f in freqs:
        for v, cents in enumerate((-8.0, 0.0, 7.0)):
            fv = f * 2.0 ** (cents / 1200.0)
            top = int(min(18, 7000.0 // fv))
            # One band-limited cycle, then read it at the voice's rate: the
            # same sum of partials as adding sines, at a fraction of the cost.
            k = np.arange(1, top + 1)
            a = (1.0 / k ** 1.2) / (1.0 + (fv * k / cutoff) ** 2)
            ph0 = rng.uniform(0, 2 * np.pi, size=top)
            cyc = np.sin(2 * np.pi * np.outer(np.arange(_TABLE) / _TABLE, k) + ph0) @ a
            cyc = np.append(cyc, cyc[0])
            pos = (fv * t) % 1.0 * _TABLE
            mono = np.interp(pos, np.arange(_TABLE + 1), cyc)
            pan = (v - 1) * 0.6
            out[:, 0] += mono * (1.0 - pan) * 0.5
            out[:, 1] += mono * (1.0 + pan) * 0.5
    swell = 1.0 + 0.08 * np.sin(2 * np.pi * 0.21 * t + rng.uniform(0, 6.28))
    env = _env(n, dur, 0.7, 0.9) * swell
    return out * env[:, None] * vel * 0.07


def bass(f: float, dur: float, vel: float = 0.8, **_) -> np.ndarray:
    n = _length(dur, 0.15)
    t = _t(n)
    mono = np.sin(2 * np.pi * f * t) + 0.3 * np.sin(4 * np.pi * f * t) + 0.08 * np.sin(6 * np.pi * f * t)
    mono = np.tanh(1.4 * mono) / np.tanh(1.4)
    mono *= _env(n, dur, 0.012, 0.09, decay=0.9, sustain=0.6) * vel * 0.45
    return _stereo(mono)


def kick(f: float = 0.0, dur: float = 0.0, vel: float = 0.8, **_) -> np.ndarray:
    """A soft heartbeat, felt more than heard."""
    n = int(0.35 * SR)
    t = _t(n)
    freq = 48.0 + 70.0 * np.exp(-t / 0.035)
    ph = 2 * np.pi * np.cumsum(freq) / SR
    mono = np.sin(ph) * np.exp(-t / 0.16) * _env(n, 1.0, 0.002, 0.1) * vel * 0.6
    return _stereo(mono)


def click(f: float = 2400.0, dur: float = 0.0, vel: float = 0.8, seed: int = 0, **_) -> np.ndarray:
    """A menu click under the input script's mouse."""
    n = int(0.05 * SR)
    t = _t(n)
    rng = np.random.default_rng(seed)
    mono = 0.5 * rng.standard_normal(n) * np.exp(-t / 0.0015)
    mono += np.sin(2 * np.pi * f * t) * np.exp(-t / 0.012)
    sos = signal.butter(2, 900, "highpass", fs=SR, output="sos")
    mono = signal.sosfilt(sos, mono) * vel * 0.2
    return _stereo(mono)


VOICES = {
    "epiano": epiano, "pulse": pulse, "pluck": pluck, "flute": flute, "bell": bell,
    "music_box": music_box, "marimba": marimba, "bleep": bleep, "bass": bass,
    "kick": kick, "click": click,
}


# --- noise beds ---------------------------------------------------------------


def crickets(n: int, seed: int, density: float = 1.0) -> np.ndarray:
    """End of the day: chirps of three pulses at ~4.4 kHz, scattered about."""
    rng = np.random.default_rng(seed)
    out = np.zeros((n, 2))
    pulse_n = int(0.012 * SR)
    pt = _t(pulse_n)
    for voice in range(3):
        f = 4300.0 + 260.0 * voice
        pan = (-0.7, 0.5, 0.1)[voice]
        tp = rng.uniform(0.0, 0.8)
        while tp < n / SR:
            for p in range(3):
                i0 = int((tp + p * 0.04) * SR)
                if i0 + pulse_n >= n:
                    break
                s = np.sin(2 * np.pi * f * pt) * np.sin(np.pi * pt / pt[-1]) ** 2
                out[i0:i0 + pulse_n, 0] += s * (1 - pan) * 0.5
                out[i0:i0 + pulse_n, 1] += s * (1 + pan) * 0.5
            tp += rng.uniform(0.7, 1.3) / density
    return out * 0.02


def filtered_noise(n: int, seed: int, lo: float | None, hi: float | None) -> np.ndarray:
    rng = np.random.default_rng(seed)
    x = rng.standard_normal((n, 2))
    if lo and hi:
        sos = signal.butter(2, [lo, hi], "bandpass", fs=SR, output="sos")
    elif hi:
        sos = signal.butter(2, hi, "lowpass", fs=SR, output="sos")
    else:
        sos = signal.butter(2, lo, "highpass", fs=SR, output="sos")
    return signal.sosfilt(sos, x, axis=0)


def scratch(n: int, seed: int, boil_hz: float = 6.0) -> np.ndarray:
    """Pencil on paper: strokes of bright noise, trembling at the boil's 6 Hz."""
    rng = np.random.default_rng(seed)
    noise = filtered_noise(n, seed + 1, 1800.0, 6500.0)
    env = np.zeros(n)
    tp = rng.uniform(0, 0.2)
    while tp < n / SR:
        length = rng.uniform(0.12, 0.45)
        i0, i1 = int(tp * SR), min(n, int((tp + length) * SR))
        if i1 > i0:
            x = np.linspace(0, 1, i1 - i0)
            env[i0:i1] += np.sin(np.pi * x) ** 1.5 * rng.uniform(0.4, 1.0)
        tp += length + rng.uniform(0.05, 0.4)
    t = _t(n)
    env *= 0.65 + 0.35 * np.sin(2 * np.pi * boil_hz * t)
    return noise * env[:, None] * 0.25


# --- the stem -----------------------------------------------------------------


class Stem:
    """One bus's audio: a dry layer and a reverb send, summed on `render`."""

    def __init__(self, seconds: float):
        self.n = int(round(seconds * SR))
        self.dry = np.zeros((self.n, 2))
        self.wet = np.zeros((self.n, 2))

    def add(self, at_s: float, sig: np.ndarray, pan: float = 0.0, gain: float = 1.0,
            send: float = 0.25) -> None:
        i0 = int(round(at_s * SR))
        if i0 >= self.n:
            return
        sig = sig[: self.n - i0].copy()
        # Every voice ends in a 6 ms fade: one whose decay outlives its buffer
        # (a bell rings on) would otherwise stop with a click.
        k = min(len(sig), int(0.006 * SR))
        if k:
            sig[-k:] *= np.linspace(1.0, 0.0, k)[:, None]
        l = np.cos((pan + 1.0) * np.pi / 4.0) * np.sqrt(2.0)
        r = np.sin((pan + 1.0) * np.pi / 4.0) * np.sqrt(2.0)
        s = sig * np.array([l, r]) * gain
        self.dry[i0:i0 + len(s)] += s * (1.0 - send)
        self.wet[i0:i0 + len(s)] += s * send

    def add_bed(self, at_s: float, bed: np.ndarray, gain: float = 1.0, send: float = 0.2,
                fade: float = 0.3) -> None:
        n = len(bed)
        f = int(fade * SR)
        e = np.ones(n)
        if f and n > 2 * f:
            ramp = np.sin(np.linspace(0, np.pi / 2, f)) ** 2
            e[:f] = ramp
            e[-f:] = ramp[::-1]
        self.add(at_s, bed * e[:, None], gain=gain, send=send)

    def render(self, ir: np.ndarray) -> np.ndarray:
        wet = np.stack([signal.fftconvolve(self.wet[:, c], ir[:, c])[: self.n] for c in range(2)],
                       axis=1)
        return self.dry + wet


def reverb_ir(seconds: float = 2.6, seed: int = 438) -> np.ndarray:
    """A clearing at dusk: a soft, dark, wide decay (noise, shaped)."""
    n = int(seconds * SR)
    t = _t(n)
    rng = np.random.default_rng(seed)
    x = rng.standard_normal((n, 2))
    early = signal.sosfilt(signal.butter(2, 7000, "lowpass", fs=SR, output="sos"), x, axis=0)
    late = signal.sosfilt(signal.butter(2, 2200, "lowpass", fs=SR, output="sos"), x, axis=0)
    mix = np.clip(t / 0.6, 0, 1)[:, None]
    ir = early * (1 - mix) + late * mix
    rt60 = seconds * 0.85
    ir *= np.exp(-6.91 * t / rt60)[:, None]
    pre = int(0.018 * SR)
    ir = np.concatenate([np.zeros((pre, 2)), ir])
    ir /= np.sqrt(np.sum(ir ** 2, axis=0, keepdims=True))
    return ir * 0.9


# --- loudness (ITU-R BS.1770-4 / EBU R128) -----------------------------------


def _k_weight(x: np.ndarray) -> np.ndarray:
    # The standard's two stages at 48 kHz: a high shelf, then the RLB high-pass.
    b1 = [1.53512485958697, -2.69169618940638, 1.19839281085285]
    a1 = [1.0, -1.69065929318241, 0.73248077421585]
    b2 = [1.0, -2.0, 1.0]
    a2 = [1.0, -1.99004745483398, 0.99007225036621]
    return signal.lfilter(b2, a2, signal.lfilter(b1, a1, x, axis=0), axis=0)


def integrated_lufs(x: np.ndarray) -> float:
    assert SR == 48_000
    y = _k_weight(x)
    block, hop = int(0.4 * SR), int(0.1 * SR)
    p = np.sum(y ** 2, axis=1)
    c = np.concatenate([[0.0], np.cumsum(p)])
    starts = np.arange(0, len(p) - block + 1, hop)
    z = (c[starts + block] - c[starts]) / block
    lk = -0.691 + 10 * np.log10(np.maximum(z, 1e-12))
    z = z[lk > -70.0]
    if len(z) == 0:
        return -70.0
    rel = -0.691 + 10 * np.log10(np.mean(z)) - 10.0
    lk = -0.691 + 10 * np.log10(z)
    return float(-0.691 + 10 * np.log10(np.mean(z[lk > rel])))


def true_peak_db(x: np.ndarray) -> float:
    up = signal.resample_poly(x, 4, 1, axis=0)
    return float(20 * np.log10(max(np.max(np.abs(up)), 1e-12)))


def limit(x: np.ndarray, ceiling_db: float = -1.5, look_ms: float = 4.0) -> np.ndarray:
    """A look-ahead brickwall: gain never above what keeps the peak at the
    ceiling, smoothed so it bends, not clicks. Measured on 4x oversampled
    peaks, so inter-sample overs are caught too."""
    ceiling = 10 ** (ceiling_db / 20)
    up = np.max(np.abs(signal.resample_poly(x, 4, 1, axis=0)), axis=1)
    peak = np.max(up[: len(x) * 4].reshape(-1, 4), axis=1)
    need = np.minimum(1.0, ceiling / np.maximum(peak, 1e-12))
    w = max(3, int(look_ms * 1e-3 * SR))
    from scipy.ndimage import minimum_filter1d, uniform_filter1d
    g = minimum_filter1d(need, size=2 * w + 1, mode="nearest")
    g = uniform_filter1d(g, size=w, mode="nearest")
    g = np.minimum(g, need)
    return x * g[:, None]
