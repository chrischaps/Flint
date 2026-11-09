"""Make the darkroom style's images (#413): 3D LUTs as 2D strips, and
tileable papers. Deterministic (fixed seeds), so a rerun writes the same
bytes.

    python tools/darkroom_images.py            # writes demo/styles/luts, papers

A strip is N slices of N x N side by side (N*N wide, N high): blue picks
the slice left to right, red runs across a slice, green down it. That is
the layout `flint_lut_<name>` reads (and Unreal's RGBTable16x1.png).
"""

from pathlib import Path

import numpy as np
from PIL import Image

N = 16
OUT = Path(__file__).resolve().parent.parent / "demo" / "styles"


def lut_grid(n=N):
    """r, g, b in 0..1 for every texel of an n-strip, shape (n, n*n)."""
    x = np.arange(n * n)
    y = np.arange(n)
    xx, yy = np.meshgrid(x, y)
    r = (xx % n) / (n - 1)
    g = yy / (n - 1)
    b = (xx // n) / (n - 1)
    return r, g, b


def save_lut(name, fn):
    r, g, b = lut_grid()
    rgb = np.stack(fn(r, g, b), axis=-1)
    rgb = np.clip(rgb, 0.0, 1.0)
    img = (rgb * 255.0 + 0.5).astype(np.uint8)
    path = OUT / "luts" / f"{name}.png"
    path.parent.mkdir(parents=True, exist_ok=True)
    Image.fromarray(img, "RGB").save(path, optimize=True)
    print(path.relative_to(OUT.parent.parent))


def luma(r, g, b):
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def smooth(x):
    return x * x * (3.0 - 2.0 * x)


def neutral(r, g, b):
    return r, g, b


def warm_print(r, g, b):
    # A fibre print on warm-tone paper: soft S-curve, shadows toward
    # umber, highlights toward cream, a touch of fade in the blacks
    s = lambda c: 0.04 + 0.92 * (0.6 * smooth(c) + 0.4 * c)
    r, g, b = s(r), s(g), s(b)
    l = luma(r, g, b)
    shadow = (1.0 - l) ** 2
    light = l**2
    r = r + 0.05 * shadow + 0.04 * light
    g = g + 0.02 * shadow + 0.015 * light
    b = b - 0.03 * shadow - 0.05 * light
    return r, g, b


def bleach(r, g, b):
    # Bleach bypass: silver left in, so the picture is half its own
    # black and white, harder, and its colour drained
    l = luma(r, g, b)
    mono = smooth(np.clip((l - 0.08) / 0.86, 0, 1))
    k = 0.55
    return (
        r * (1 - k) + mono * k,
        g * (1 - k) + mono * k,
        b * (1 - k) + mono * k * 1.02,
    )


def cyanotype(r, g, b):
    # Prussian blue: tone becomes ink, white stays paper
    l = luma(r, g, b)
    t = smooth(np.clip(l * 1.08 - 0.02, 0, 1))
    deep = np.array([0.05, 0.16, 0.36])
    pale = np.array([0.93, 0.95, 0.97])
    return tuple(deep[i] + (pale[i] - deep[i]) * t for i in range(3))


def periodic_noise(size, scale, seed, aniso=(1.0, 1.0)):
    """Band-limited noise that tiles: white noise filtered in frequency."""
    rng = np.random.default_rng(seed)
    white = rng.standard_normal((size, size))
    f = np.fft.fftfreq(size)
    fx, fy = np.meshgrid(f, f)
    radius = np.sqrt((fx * aniso[0]) ** 2 + (fy * aniso[1]) ** 2)
    spectrum = np.exp(-((radius * scale) ** 2))
    out = np.real(np.fft.ifft2(np.fft.fft2(white) * spectrum))
    out -= out.mean()
    out /= out.std() + 1e-9
    return out


def save_paper(name, base, layers, seed):
    size = 256
    tone = np.zeros((size, size))
    for i, (scale, amount, aniso) in enumerate(layers):
        tone += amount * periodic_noise(size, scale, seed + i, aniso)
    rgb = np.clip(np.array(base)[None, None, :] * (1.0 + tone[..., None]), 0, 1)
    img = (rgb * 255.0 + 0.5).astype(np.uint8)
    path = OUT / "papers" / f"{name}.png"
    path.parent.mkdir(parents=True, exist_ok=True)
    Image.fromarray(img, "RGB").save(path, optimize=True)
    print(path.relative_to(OUT.parent.parent))


def main():
    save_lut("neutral", neutral)
    save_lut("warm_print", warm_print)
    save_lut("bleach", bleach)
    save_lut("cyanotype", cyanotype)
    # (scale of the blur in texels, amount, stretch across x and y)
    save_paper(
        "cotton",
        [0.96, 0.93, 0.86],
        [(18.0, 0.020, (1, 1)), (4.0, 0.022, (1, 1)), (1.2, 0.018, (1, 1))],
        seed=413,
    )
    save_paper(
        "kraft",
        [0.74, 0.60, 0.44],
        [(24.0, 0.045, (1, 1)), (3.0, 0.050, (1, 1)), (1.0, 0.035, (1, 1))],
        seed=414,
    )
    save_paper(
        "washi",
        [0.95, 0.94, 0.90],
        [(30.0, 0.025, (1, 1)), (8.0, 0.05, (0.15, 1.0)), (2.5, 0.03, (1.0, 0.2))],
        seed=415,
    )


if __name__ == "__main__":
    main()
