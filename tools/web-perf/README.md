# Web perf: the web player's `?perf=1` readout in desktop Chrome (CDP) or Firefox (BiDi) (#381, #430)

`run.mjs` plays each scene of a bundle in headed Chrome, a page at a time,
with the canvas fixed at one size in device pixels, and reads the corner
readout: frame times, each pass's GPU time, the WebGPU calls a frame. It
writes one JSON and one screenshot per page and a Markdown table over them
all. Node 22+ (built-in `fetch` and `WebSocket`, no npm) and Chrome; nothing
else. For the phone, see `../phone-perf/`.

```bash
# 1. a game folder: here #270's, the ASCII scene and two variants of it
python -I tools/phone-perf/ascii_variants.py flint-examples/ascii_entities.scene.toml <work>/asciiweb .

# 2. the player and the bundle together, as play.chaps.dev has them (trunk, web-release)
flint export web <work>/asciiweb --out <work>/site      # or --player <a trunk dist> to skip the build

# 3. every scene at DPR 1 and 2, the canvas 1920x1080 device pixels at both
node tools/web-perf/run.mjs <work>/site games/asciiweb/asciiweb.flintpack \
  ascii_none,ascii_entities,ascii_full --dpr 1,2 --passes 'Style mask,ASCII' --out <work>/out
```

`<work>/out/table.md` is then `docs/design/web.md`'s browser table for the
stylize passes ("Stylize passes (#270)"). On 2026-11-12 (RTX 4070 laptop,
Chrome 157) it read:

| page | canvas | scene | Style mask ms | ASCII ms | GPU frame ms | GPU timed | bind groups a frame | passes a frame | frame p50 ms |
|---|---|---|---|---|---|---|---|---|---|
| DPR 1, ascii_none | 1920x1080 | 1920x1080 |  |  | 3.15--3.96 | vsync, idle clocks (17--24% busy) | 0.0 | 17.0 | 16.7 |
| DPR 1, ascii_entities | 1920x1080 | 1920x1080 | 0.04 | 0.40--0.53 | 4.18--4.41 | vsync, idle clocks (25--26% busy) | 0.0 | 26.0 | 16.7 |
| DPR 1, ascii_full | 1920x1080 | 1920x1080 | 0.03--0.05 | 0.72--0.80 | 4.38--4.43 | vsync, idle clocks (26--27% busy) | 0.0 | 26.0 | 16.7 |
| DPR 2, ascii_none | 1920x1080 | 1440x810 |  |  | 2.98--3.07 | vsync, idle clocks (18% busy) | 0.0 | 18.0 | 16.7 |
| DPR 2, ascii_entities | 1920x1080 | 1440x810 | 0.02--0.03 | 0.99--1.00 | 4.24--4.25 | vsync, idle clocks (25% busy) | 0.0 | 27.0 | 16.7 |
| DPR 2, ascii_full | 1920x1080 | 1440x810 | 0.02 | 0.80--1.23 | 3.20--3.89 | vsync, idle clocks (19--23% busy) | 0.0 | 27.0 | 16.7 |

Two earlier runs that day put per-entity ASCII at DPR 1 at 0.34--0.41 and
0.54--0.60 ms: other jobs share the GPU. A page presents on the browser's
frame, so its GPU times are always taken under vsync, and the column says
when the GPU idled at low clocks and read high (#380). Run it more than once,
and compare a pass against a control page from the same run.

The first argument is a folder to serve (on 127.0.0.1, for the run only) or
the URL of a site already up. A scene may carry its own query after `+`:
`ascii_none+style=invert`.

| Flag | Default | |
|---|---|---|
| `--out <dir>` | (required) | `<scene>@dpr<d>.json` and `.png` per page, `summary.json`, `table.md` |
| `--dpr 1,2` | `1` | each DPR plays every scene in its own Chrome, launched with `--force-device-scale-factor`, so the display's own scale doesn't matter. Not CDP's `Emulation.setDeviceMetricsOverride`: it changes `devicePixelRatio`, but winit sizes the canvas from a ResizeObserver's device-pixel box, which kept the display's scale. A page whose canvas isn't `--size` is flagged `(!)` and the run exits 1 |
| `--size WxH` | `1920x1080` | the canvas in device pixels at every DPR (CSS size = size / DPR, the page's `width=&height=`) |
| `--seconds N` | `25` | how long a page plays before the first read |
| `--reads N` | `2` | reads, 10 s apart: the readout covers the last 10 s, so each is its own window |
| `--passes a,b` | every pass seen | the GPU passes the table has a column for |
| `--query '&k=v'` | | more of the page's query on every page (`&fps=native`, `&scale=1`, `&perf=cpu`...) |
| `--begin auto\|click` | `auto` | `auto` adds `?begin=auto`, which lifts the Begin gate with no click; `click` clicks Begin over CDP with a user gesture instead, which also opens the sound |
| `--rounds N` | `1` | plays every scene once a round, N rounds, in the same browser: `ascii_none,ascii_full --rounds 4` is none, full, none, full... Heat or another job on the GPU then shows as a trend across rounds, not as a step between scenes, so read each scene against its own round's control. Labels gain `-r<n>` and the table a row per round. `../phone-perf/submits.mjs` alternates the same way on the phone (#568, #1318) |
| `--port N` | `9381` | Chrome's devtools port; pick another to run two at once |
| `--chrome <path>` | the usual install, or `CHROME` | |
| `--browser chrome\|firefox` | `chrome` | `firefox` drives headed Firefox over WebDriver BiDi on `--port` (it has no CDP), on a fresh profile whose `layout.css.devPixelsPerPx` is the DPR, as Chrome's `--force-device-scale-factor` is (#430) |
| `--firefox <path>` | the usual install, or `FIREFOX` | |
| `--prefs k=v,k=v` | | more Firefox prefs for the run's profile, or its own overridden (`true`, `false` and integers are typed). `layout.frame_rate=60` is the one to compare builds with (below) |
| `--trace` | off | from the moment the gate lifts, keeps the time of every frame the browser gives the page (a `requestAnimationFrame` loop beside the player's, `trace.mjs`), and writes it to each page's JSON as `trace`: the frames each second (`fps`), how many seconds ran at 45 fps or less (`slowSeconds`), and the spells of slow frames, each with when it began and how long it lasted. The table gains a column. It's how to tell a frame's own work running long from the browser missing vsync (#962) |

Each page's JSON keeps the readout's text and its copyable line at every read
(`reads[].text`, `reads[].line`), the line parsed into numbers
(`reads[].parsed`, `perfline.mjs`), what the page saw (DPR, viewport, canvas,
user agent, whether the adapter has `timestamp-query`) and
`window.flintGpuCalls`' totals, at the end (`calls`) and at each read
(`reads[].calls`). With two reads or more, `between` is the last read's totals
less the first's, printed to the log too: the readout rounds a frame's count to
one decimal, and this is exact (`submit` is one a frame, so it counts the
frames). `"createBindGroup": 0` over 1200 frames is #565's reading. A page whose
gate never lifts is recorded with its status line as `error`, and the run exits 1.

**What it needs from the machine.** Chrome draws WebGPU only headed, and gives
no frames to a window that is off screen or covered, so the window opens at
0,0 with the throttling flags off (and `CalculateNativeWinOcclusion` disabled):
leave it be while it runs. Other jobs on the GPU move the numbers; run twice
and look at a pass the change doesn't touch (SSAO, Bloom) to spot a contended
run. Tests for the line parser and the trace: `node --test tools/web-perf/perfline.test.mjs tools/web-perf/trace.test.mjs`.

## GPU-bound pages: the mean frame (#1318)

One frame's time snaps to whole refreshes, so a pass costing 2 ms reads as
nothing or as half the frame rate, depending on where the frame sat. The mean
doesn't: the readout's `mean` row (the line's `mean N ms a frame` part, the
table's `mean ms a frame` column) is the window's span over its frames. When
the frames are unpaced and the GPU is what holds them, that is the GPU's time
a frame, and the row says `GPU-bound`, with what it judged from:

```text
mean 38.40 ms a frame, GPU-bound (guess: unpaced, player 2.48 ms, refresh 16.67 ms)
```

That is Float with `style=kuwahara` on a 3840x2160 canvas, `fps=native`, in
Chrome 157 on the RTX 4070 laptop (2026-11-20, `--rounds 3`): its frames read
p50 33.4 and p90 50.0, snapped to two and three refreshes, while the mean was
38.40 to 38.78 ms, timestamps said 38.2 to 39.1, and the page counted 258 to
261 submissions between reads 10 s apart. The same page at `fps=60` read the
same mean with no flag (paced), and Float alone held 16.67 with none (the
display's frame). The book's Profiling guide has the native runs.

It is a guess from the CPU side, so it prints its inputs. It says GPU-bound
only when all three hold:

- **unpaced**: no `?fps=` pacer (`fps=native`; the phone's default `auto`
  paces, and so does a fixed rate). A paced frame waits for the pacer.
- **the player's work** (every CPU phase but the swapchain's waits, `acquire`
  and `present`) is under half the frame. Otherwise the frame is the CPU's.
- **under vsync, the frames miss the refresh**: the mean is at least 1.15
  times the refresh period. A page that holds 60 Hz is the display's, however
  light its GPU. The refresh is the pacer's, or, unpaced, read from the same
  rAF intervals (the slowest of 60, 75, 90 ... 240 Hz whose multiples explain
  nine in ten of them). Without vsync (the desktop's `FLINT_VSYNC=0`) nothing
  holds the frames but the GPU, and the line says `no vsync`.

What it can't see is time the browser's own main thread or the compositor
takes: a frame held there reads as GPU-bound too. Where timestamps work,
the GPU frame time beside it is a check. On the phone, where Mali's read 0
(#568), it is the only GPU number there is: `?perf=1&fps=native&scale=1`
makes the page GPU-bound, and `--rounds` (or `submits.mjs`) alternates the
pages so each is read against its own round's control.

## Firefox runs at half rate before it runs at 60 (#962)

**In Firefox, compare builds with `--prefs layout.frame_rate=60`.** Without
it, Firefox paces frames from the display's hardware vsync. On this machine
(RTX 4070 laptop, Firefox 159, Windows 11) every page then spends its first
14 to 18 s after the gate at 33 to 38 fps (mostly 33 ms frames with a 17
here and there), and only then holds 60. A second page in the same Firefox
ran at 60 for 3 s, then at half rate for the other 41 s. The readout's p99
reads 31 to 33 ms in any window that covers a spell like that. With
`layout.frame_rate=60`, Firefox's own 60 Hz timer drives the refresh, and
every page held 60 from the start, apart from #1349's freeze (below). Chrome
doesn't do this. Whether a Firefox a person opens does it too is unmeasured:
every run here was a fresh profile driven over BiDi.

This was #949's "Sun Prints hitches to 33 ms on wgpu 30". On 2026-11-20 I
ran six alternated rounds with `--seconds 15 --reads 4 --trace`. They
covered the last wgpu 23 build (15a256d9), the wgpu 30 bump (09fa0efe), main
(3969a3b7), and a page with no Flint at all: a WebGPU clear, or only a
`requestAnimationFrame` loop. The time at 45 fps or less was:

| page | seconds at 45 fps or less, six rounds | p99 over 25 ms in the 15-35 s windows #949 read |
|---|---|---|
| Sun Prints, wgpu 23 | 16 17 17 17 17 18 | 6 of 6 |
| Sun Prints, wgpu 30 | 17 17 17 18 18 18 | 6 of 6 |
| Sun Prints, main | 15 16 18 18 18 18 | 6 of 6 |
| no Flint, a WebGPU clear | 14 17 17 18 18 18 | (no readout) |

With `layout.frame_rate=60` (three rounds more), Sun Prints read p99 17.1 to
17.5 ms in 22 of 24 windows across both builds. The other two, 33.3 and
49.6 ms, came from one wgpu 23 run. Turning
`layout.lower_priority_refresh_driver_during_load` or
`gfx.webrender.compositor` off changed nothing.

**What each kind of slow looks like**, so a run can tell them apart:

- **Firefox's hardware vsync.** The frame's p50 or p90 sits at 30 to 33 ms,
  with 340 to 550 frames in 10 s. CPU a frame doesn't move (1.0 to 1.4 ms
  here, as at 60), and the page with no Flint does the same. `--trace`'s
  `fps` reads 33 to 38 a second.
- **A busy machine.** Several frames' CPU phases stretch together
  (`input`, `scripts`, `render`, `hud`, all at once), as in #949's run. The
  frames stay on vsync until the CPU a frame nears the interval. In these
  rounds, 27 compiler processes and 56% machine CPU moved Sun Prints' CPU a
  frame from 1.1 to 1.4 to 1.6 ms, and the half-rate spell lasted 16 s, as
  on an idle machine. Log the load beside a run (process counts and free
  memory) before blaming it.
- **The frame's own work.** One phase grows, and a slow spell in the
  trace lines up with it. Compare against a control page from the same run.

`--trace` also shows what the readout leaves out. In 7 of 25 Firefox runs of
Sun Prints, Firefox gave the page no frame for 1.7 to 2.0 s about 0.1 s
after the gate lifted: 5 of 7 runs on main, 1 of 9 on each of the other two
builds. The player counts a hitch that early as startup, apart from the
readout's count, which reads 0. #1349 follows it up.
