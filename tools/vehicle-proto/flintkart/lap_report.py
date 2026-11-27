"""Read lap_probe.rhai's log lines (VP/VLAP) from a playtest's stderr: each
kart's laps, top speed, wall time and the tightest corner it held (the
smallest v / yaw-rate over a 0.5 s window at more than 10 m/s with no wall
contact, after GO), as the prototype measures them. Writes a JSON summary."""
import json, math, sys

def read(path):
    rows, laps, start = {}, {}, None
    for line in open(path, encoding='utf-8', errors='replace'):
        i = line.find('VP ')
        if i >= 0 and 'VLAP' not in line:
            f = line[i + 3:].split()
            t, name = float(f[0]), f[1]
            x, y, z, s, yaw = map(float, f[2:7])
            rows.setdefault(name, []).append((t, x, y, z, s, yaw, f[7] == 'true', f[8]))
        i = line.find('VLAP ')
        if i >= 0:
            f = line[i + 5:].split()
            laps.setdefault(f[1], []).append(round(float(f[3]), 3))
        i = line.find('VSTART ')
        if i >= 0:
            start = float(line[i + 7:].split()[0])
    return rows, laps, start

def measure(rows, start):
    top, wall_t, tight = 0.0, 0.0, (1e9, 0.0)
    win = []
    prev = None
    drift_t = 0.0
    for r in rows:
        t, x, y, z, s, yaw, wall, drift = r
        if t < start:
            prev = r
            continue
        dt = t - prev[0] if prev else 0.0
        if prev and dt > 0:
            hs = math.hypot(x - prev[1], z - prev[3]) / dt
            top = max(top, hs)
        if wall:
            wall_t += dt
        if drift in ('drifting', 'hop'):
            drift_t += dt
        win.append((t, s, yaw, wall))
        win = [w for w in win if t - w[0] <= 0.5 + 1e-4]
        if len(win) > 2 and all((not w[3]) and w[1] > 10 for w in win):
            a, b = win[0], win[-1]
            d = (b[2] - a[2] + 540) % 360 - 180
            rate = abs(math.radians(d)) / (b[0] - a[0])
            v = sum(w[1] for w in win) / len(win)
            if rate > 1e-3 and v / rate < tight[0]:
                tight = (v / rate, v)
        prev = r
    return dict(top_speed=round(top, 2), wall_time=round(wall_t, 2), drift_time=round(drift_t, 2),
                tight_radius=round(tight[0], 2), tight_speed=round(tight[1], 2))

out = {}
for path in sys.argv[1:-1]:
    rows, laps, start = read(path)
    res = {}
    for name, rs in sorted(rows.items()):
        m = measure(rs, start or 4.0)
        m['laps'] = laps.get(name, [])
        res[name] = m
    out[path.split('\\')[-1].split('/')[-1]] = res
    for name, m in res.items():
        if m['laps']:
            print(path.split('/')[-1], name, m)
# Blaze's second lap, for the presentation
rows, laps, start = read(sys.argv[1])
json.dump(out, open(sys.argv[-1], 'w'), indent=1)
