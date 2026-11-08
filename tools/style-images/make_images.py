"""Make the demo paper and stroke images for #390.

Both are synthesized (not scans): a tileable cold-press paper, shaded as a
scanner's raking light would show its relief, and a strip of four graphite
strokes. Deterministic: fixed seeds.

    python make_images.py <demo dir>
"""
import sys
import numpy as np
from PIL import Image

out = sys.argv[1]
rng = np.random.default_rng(390)


def band(n, lo, hi, aniso=(1.0, 1.0)):
    """Periodic band-passed noise on an n x n torus, unit variance."""
    w = rng.standard_normal((n, n))
    f = np.fft.fft2(w)
    fy = np.fft.fftfreq(n)[:, None] * aniso[1]
    fx = np.fft.fftfreq(n)[None, :] * aniso[0]
    r = np.sqrt(fx * fx + fy * fy) * n
    g = np.exp(-((np.log(np.maximum(r, 1e-6)) - np.log((lo * hi) ** 0.5)) ** 2)
               / (2 * (np.log(hi / lo) / 2.5) ** 2))
    g[0, 0] = 0
    v = np.real(np.fft.ifft2(f * g))
    return v / v.std()


# --- paper: 512 x 512, tileable ----------------------------------------------
N = 512
# Rounded nodules (cold press), a finer tooth, and broad cockling
nod = band(N, 30, 70)
nod = np.tanh(nod * 0.9)          # flattened tops, rounded hills
tooth = band(N, 90, 200)
broad = band(N, 2, 6)
h = 0.62 * nod + 0.25 * tooth + 0.35 * broad
# Fibres: short faint strands, wrapped round the tile
fib = np.zeros((N, N))
for _ in range(260):
    x0, y0 = rng.uniform(0, N, 2)
    ang = rng.uniform(0, np.pi)
    length = rng.uniform(10, 40)
    t = np.linspace(0, 1, int(length * 2))
    bend = rng.uniform(-0.3, 0.3)
    xs = (x0 + np.cos(ang + bend * t) * length * t) % N
    ys = (y0 + np.sin(ang + bend * t) * length * t) % N
    fib[ys.astype(int), xs.astype(int)] += rng.uniform(0.4, 1.0)
# soften the fibres by a periodic blur
k = np.fft.fft2(fib)
fy = np.fft.fftfreq(N)[:, None]
fx = np.fft.fftfreq(N)[None, :]
fib = np.real(np.fft.ifft2(k * np.exp(-(fx * fx + fy * fy) * (N * N) / (2 * 140.0 ** 2) * 4)))
fib /= fib.max() + 1e-9
h = h + 0.5 * fib
# Raking light from the upper left: luma follows the slope, as a scan shows it
gy, gx = np.gradient(np.pad(h, 1, mode='wrap'))
gx, gy = gx[1:-1, 1:-1], gy[1:-1, 1:-1]
shade = -(gx * 0.7 + gy * 0.7)
lum = 0.90 + 0.055 * shade / shade.std() + 0.015 * h / h.std()
lum += rng.normal(0, 0.006, (N, N))   # scanner noise
cream = np.array([1.0, 0.975, 0.93])
rgb = np.clip(lum[..., None] * cream, 0, 1)
Image.fromarray((rgb * 255 + 0.5).astype(np.uint8), 'RGB').save(out + '/papers/cold_press.png', optimize=True)

# --- strokes: four graphite strokes, 512 x 96 (24 px a row) ------------------
W, RH, ROWS = 512, 24, 4
strip = np.zeros((RH * ROWS, W))
yy, xx = np.mgrid[0:RH, 0:W].astype(float)
for r in range(ROWS):
    s = rng.uniform(0, 1000)
    u = xx / (W - 1)
    # A slight arc and a drift, the hand's
    centre = RH / 2 + (u - 0.5) ** 2 * rng.uniform(-6, 6) + np.sin(u * np.pi * rng.uniform(1, 2) + s) * 1.2
    # Pressure: a firm start, a long middle, a lifting tail
    start = rng.uniform(0.03, 0.08)
    lift = rng.uniform(0.7, 0.9)
    pressure = np.clip(u / start, 0, 1) ** 0.6 * np.clip((1 - u) / (1 - lift), 0, 1) ** 1.4
    pressure *= 0.85 + 0.15 * np.sin(u * 7 + s)
    width = 4.0 + 3.0 * pressure            # px half-width at full pressure
    d = np.abs(yy - centre) / np.maximum(width, 0.5)
    body = np.clip(1 - d ** 2.2, 0, 1)
    # Graphite grain: streaks along the stroke (noise stretched in x) and
    # paper tooth that the lead skips
    streak = rng.standard_normal((RH, W // 16 + 2))
    streak = np.array(Image.fromarray(streak.astype(np.float32), 'F').resize((W, RH), Image.BICUBIC))
    tooth = rng.uniform(0, 1, (RH, W))
    tooth = np.array(Image.fromarray(tooth.astype(np.float32), 'F').resize((W // 2, RH // 2), Image.BILINEAR).resize((W, RH), Image.BILINEAR))
    grain = np.clip(0.55 + 0.25 * streak + 0.6 * (tooth - 0.5) * 2.2, 0, 1)
    ink = body * pressure * grain
    ink = np.clip(ink * 1.25, 0, 1) ** 1.1
    strip[r * RH:(r + 1) * RH] = ink
lead = np.array([0.18, 0.18, 0.21])
rgba = np.zeros((RH * ROWS, W, 4))
rgba[..., :3] = lead
rgba[..., 3] = strip
Image.fromarray((rgba * 255 + 0.5).astype(np.uint8), 'RGBA').save(out + '/strokes/graphite.png', optimize=True)
print('paper', lum.mean(), lum.std(), 'strip ink p99', np.percentile(strip, 99))
