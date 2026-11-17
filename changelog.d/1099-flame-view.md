### `flint frame open` opens a scope trace: a profiler tab with the frame strip, a flame chart and scope statistics (#1099)
**What changed.** `flint frame open` takes a trace file (what `--trace`,
`--trace-on-hitch` or Shift+F3 wrote) as well as a capture. A trace opens on a
**Profiler** tab: every frame as a strip in the F2 graph's colours with
hitches marked, the chosen frames as a flame chart per thread with the script
callbacks and GPU passes beneath, and each scope's count, total, mean, p90 and
worst over them. A capture taken while recording, which holds `trace.json`,
opens with both tabs, and P switches between them. New flags under
**Profiler (a trace)**: `--profile` (open on that tab), `--trace-frame
N|A..B|hitch|worst`, `--scope NAME` (mark its calls and plot its time across
frames) and `--compare` (tint each bar by how much longer its path took than
in the median frame). `--screenshot` draws it headless. Captures and traces
are read as before; nothing is written.
**What to do.** Nothing. Open a hitch's trace with `flint frame open
traces/<scene>-hitch-t<time>.json`.
