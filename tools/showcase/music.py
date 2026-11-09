"""Build the showcase film's music (#438): one WAV per cut, as long as the cut.

    python tools/showcase/music.py                 # every cut in tools/showcase/cuts/
    python tools/showcase/music.py reel            # one
    python tools/showcase/music.py --out D:/film   # somewhere else (default ~/flint-film/music)
    python tools/showcase/music.py --verify        # build twice, compare every byte

For each cut list `cuts/<cut>.toml` it

  1. arranges the score (score.py) to the cut's shots and synthesizes six
     stems (synth.py) into <out>/stems/<cut>/,
  2. writes the suite and its mix automation beside this script,
     `music/<cut>.suite.toml` and `music/<cut>.events.toml`,
  3. renders them with Flint's own offline mixer:
     `flint render-suite music/<cut>.suite.toml --script music/<cut>.events.toml`,
  4. masters the result to -14 LUFS integrated (EBU R128 / BS.1770-4, what
     YouTube and Instagram normalize to) under a -1.5 dBTP ceiling, and
     writes <out>/many-hands-<cut>.wav (48 kHz, 24-bit) and a report,
     <out>/many-hands-<cut>.json, with its length in samples and frames.

The WAV is exactly the cut's length: bars x 2.4 s, so bars x 72 frames at
30 fps. Nothing in it comes from anywhere else: every sound is synthesized.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path

import numpy as np
from scipy.io import wavfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))

import score  # noqa: E402
import synth  # noqa: E402

TARGET_LUFS = -14.0
CEILING_DB = -1.5


def write_wav24(path: Path, x: np.ndarray) -> None:
    import wave
    q = np.clip(np.round(x * 8388607.0), -8388608, 8388607).astype("<i4")
    raw = q.reshape(-1).view(np.uint8).reshape(-1, 4)[:, :3].tobytes()
    path.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(path), "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(3)
        w.setframerate(synth.SR)
        w.writeframes(raw)


def flint_bin(explicit: str | None) -> list[str]:
    if explicit:
        return [explicit]
    exe = "flint.exe" if os.name == "nt" else "flint"
    target = Path(os.environ.get("CARGO_TARGET_DIR") or ROOT / "target")
    for d in ("release", "debug"):
        p = target / d / exe
        if p.exists():
            return [str(p)]
    return ["cargo", "run", "--release", "--quiet", "--bin", "flint", "--"]


def master(x: np.ndarray) -> tuple[np.ndarray, dict]:
    y = x.copy()
    for _ in range(4):
        lufs = synth.integrated_lufs(y)
        y = y * 10 ** ((TARGET_LUFS - lufs) / 20)
        y = synth.limit(y, CEILING_DB)
    lufs = synth.integrated_lufs(y)
    return y, {
        "integrated_lufs": round(lufs, 2),
        "true_peak_dbtp": round(synth.true_peak_db(y), 2),
        "sample_peak_dbfs": round(float(20 * np.log10(np.max(np.abs(y)))), 2),
        "clipped_samples": int(np.sum(np.abs(y) >= 1.0)),
    }


def plot(cut, stems: dict, final: np.ndarray, path: Path) -> None:
    """A sheet to read the track by eye: the cut's shots, a spectrogram of the
    master, and each bus's level, all on one time axis."""
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    from scipy import signal

    ink, paper, faint = "#2b2622", "#f4efe6", "#b9ad9c"
    colours = {"foundation": "#7a4e2d", "harmony": "#c58b4a", "world_voice": "#5f8a6b",
               "home_theme": "#2f5d8a", "child_motif": "#9b6fb0", "texture": "#8c8c84"}
    fam_col = {"plain": "#d9d2c5", "type": "#9fc79a", "edges": "#a9cfa4", "entities": "#b8d6a0",
               "mask": "#e2c48f", "story": "#d6b98c", "reveal": "#c7d9a8", "pencil": "#c9c2b3",
               "watercolor": "#a9c4d6", "looks": "#d8b0a0", "user": "#c2b0d8", "menu": "#e0d0a8",
               "hands": "#e8b98a", "coda": "#ece6db"}
    secs = cut.seconds
    fig, axes = plt.subplots(3, 1, figsize=(16, 7.2), sharex=True, facecolor=paper,
                             gridspec_kw={"height_ratios": [0.55, 2.2, 1.6], "hspace": 0.06})
    for ax in axes:
        ax.set_facecolor(paper)
        for s in ax.spines.values():
            s.set_visible(False)
        ax.tick_params(colors=ink, labelsize=8)
    top, spec, lev = axes
    for s in cut.shots:
        t0, w = s.start * score.BEAT_S, s.length * score.BEAT_S
        top.add_patch(plt.Rectangle((t0, 0), w, 1, color=fam_col.get(s.family, faint), lw=0))
        top.plot([t0, t0], [0, 1], color=paper, lw=1.2)
        label = s.take if s.beat == "10_looks" else s.beat.split("_", 1)[-1]
        if w >= 1.1:
            top.text(t0 + w / 2, 0.5, label.replace("_", " "), ha="center", va="center",
                     fontsize=7 if w < 2.5 else 8, color=ink, rotation=90 if w < 2 else 0)
    top.set_ylim(0, 1)
    top.set_yticks([])
    mono = final.mean(axis=1)
    f, t, z = signal.spectrogram(mono, fs=synth.SR, nperseg=4096, noverlap=3072)
    keep = (f > 30) & (f < 12000)
    db = 10 * np.log10(z[keep] + 1e-12)
    spec.pcolormesh(t, f[keep], db, shading="auto", cmap="magma", vmin=db.max() - 85,
                    vmax=db.max())
    spec.set_yscale("log")
    spec.set_ylim(40, 12000)
    spec.set_ylabel("Hz", color=ink, fontsize=8)
    hop = synth.SR // 20
    for bus, x in stems.items():
        p = np.mean(x ** 2, axis=1)
        n = len(p) // hop
        r = 10 * np.log10(p[: n * hop].reshape(n, hop).mean(axis=1) + 1e-12)
        r = r - 10 * np.log10(np.mean(np.concatenate([s ** 2 for s in stems.values()])) + 1e-12)
        lev.plot(np.arange(n) * hop / synth.SR, np.maximum(r, -50), color=colours[bus], lw=1.1,
                 label=bus.replace("_", " "))
    lev.set_ylim(-50, 12)
    lev.set_ylabel("dB", color=ink, fontsize=8)
    lev.legend(loc="lower left", ncol=6, fontsize=8, frameon=False, labelcolor=ink)
    lev.set_xlim(0, secs)
    lev.set_xlabel("seconds", color=ink, fontsize=8)
    for bar in range(int(cut.bars) + 1):
        for ax in (spec, lev):
            ax.axvline(bar * 2.4, color=faint if ax is lev else "#ffffff", lw=0.4,
                       alpha=0.5 if ax is lev else 0.12)
    fig.suptitle(f"Many Hands: the {cut.name} cut, {int(cut.bars)} bars, {secs:.1f} s",
                 color=ink, fontsize=11, x=0.125, ha="left")
    fig.savefig(path, dpi=110, bbox_inches="tight", facecolor=paper)
    plt.close(fig)


def build(cut_path: Path, out: Path, flint: list[str], quiet: bool = False,
          sheet: bool = False) -> dict:
    cut = score.load_cut(cut_path)
    say = (lambda *a: None) if quiet else print
    say(f"[{cut.name}] {cut.bars} bars, {cut.seconds:.1f} s, {len(cut.shots)} shots")

    stems = score.Arranger(cut).build()
    peak = max(float(np.max(np.abs(s))) for s in stems.values())
    scale = 0.5 / peak  # one gain for all six keeps their balance
    rel = Path("stems") / cut.name
    for bus, s in stems.items():
        write_wav24(out / rel / f"{bus}.wav", s * scale)
        rms = float(np.sqrt(np.mean((s * scale) ** 2)))
        say(f"  stem {bus:12s} rms {20 * np.log10(max(rms, 1e-9)):6.1f} dBFS")

    suite = HERE / "music" / f"{cut.name}.suite.toml"
    events = HERE / "music" / f"{cut.name}.events.toml"
    suite.parent.mkdir(exist_ok=True)
    suite.write_text(score.suite_toml(cut, rel.as_posix()), encoding="utf-8", newline="\n")
    events.write_text(score.events_toml(cut), encoding="utf-8", newline="\n")

    raw = out / f"many-hands-{cut.name}.raw.wav"
    cmd = flint + ["render-suite", str(suite), "--script", str(events), "--output", str(raw),
                   "--base-dir", str(out), "--duration-bars", str(int(cut.bars))]
    run = subprocess.run(cmd, capture_output=True, text=True)
    if run.returncode != 0:
        sys.stderr.write(run.stdout + run.stderr)
        raise SystemExit(f"flint render-suite failed for {cut.name}")
    fired = sum(1 for line in run.stdout.splitlines() if line.startswith("fired"))
    late = sum(1 for line in run.stdout.splitlines() if "(LATE)" in line)

    sr, mix = wavfile.read(raw)
    assert sr == synth.SR, sr
    mix = mix.astype(np.float64)
    want = int(round(cut.seconds * synth.SR))
    if len(mix) != want:
        raise SystemExit(f"{cut.name}: render is {len(mix)} samples, the cut is {want}")
    final, meter = master(mix)
    wav = out / f"many-hands-{cut.name}.wav"
    write_wav24(wav, final)
    raw.unlink()
    if sheet:
        plot(cut, {b: x * scale for b, x in stems.items()}, final,
             out / f"many-hands-{cut.name}.png")

    report = {
        "cut": cut.name,
        "file": wav.name,
        "bars": int(cut.bars),
        "seconds": round(cut.seconds, 3),
        "samples": len(final),
        "sample_rate": synth.SR,
        "fps": cut.fps,
        "frames": int(round(cut.seconds * cut.fps)),
        "frames_30fps": int(round(cut.seconds * 30)),
        "frames_60fps": int(round(cut.seconds * 60)),
        "events_fired": fired,
        "events_late": late,
        **meter,
        "sha256": hashlib.sha256(wav.read_bytes()).hexdigest(),
        "shots": [
            {"beat": s.beat, "take": s.take, "bar": s.start / 4, "bars": s.length / 4,
             "at_s": round(s.start * score.BEAT_S, 3),
             "frame": int(round(s.start * score.BEAT_S * cut.fps)),
             "frame_30fps": int(round(s.start * 18))}
            for s in cut.shots
        ],
    }
    (out / f"many-hands-{cut.name}.json").write_text(json.dumps(report, indent=2) + "\n",
                                                     encoding="utf-8")
    say(f"  {wav}: {report['seconds']} s = {report['frames']} frames at {cut.fps} fps, "
        f"{meter['integrated_lufs']} LUFS, {meter['true_peak_dbtp']} dBTP, "
        f"{meter['clipped_samples']} clipped, {fired} events ({late} late)")
    return report


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("cuts", nargs="*", help="cut names (default: every cuts/*.toml)")
    ap.add_argument("--out", default="~/flint-film/music")
    ap.add_argument("--flint", help="the flint binary (default: target/release, then cargo run)")
    ap.add_argument("--verify", action="store_true", help="build twice and compare the WAVs")
    ap.add_argument("--plot", action="store_true",
                    help="also draw <out>/many-hands-<cut>.png: shots, spectrogram, bus levels")
    a = ap.parse_args()
    out = Path(os.path.expanduser(a.out))
    names = a.cuts or sorted(p.stem for p in (HERE / "cuts").glob("*.toml"))
    flint = flint_bin(a.flint)
    for name in names:
        first = build(HERE / "cuts" / f"{name}.toml", out, flint, sheet=a.plot)
        if a.verify:
            again = build(HERE / "cuts" / f"{name}.toml", out / "verify", flint, quiet=True)
            same = first["sha256"] == again["sha256"]
            print(f"  verify: {'byte-identical' if same else 'DIFFERENT'}")
            if not same:
                raise SystemExit(1)


if __name__ == "__main__":
    main()
