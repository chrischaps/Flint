# Web perf: the web player's `?perf=1` readout in desktop Chrome over CDP (#381)

`run.mjs` plays each scene of a bundle in headed Chrome, a page at a time,
with the canvas fixed at one size in device pixels, and reads the corner
readout: frame times, each pass's GPU time, the WebGPU calls a frame. It
writes one JSON and one screenshot per page and a Markdown table over them
all. Node 22+ (built-in `fetch` and `WebSocket`, no npm) and Chrome; nothing
else. For the phone, see `../phone-perf/`.

```bash
# 1. a game folder: here #270's, the ASCII scene and two variants of it
python -I tools/phone-perf/ascii_variants.py examples/ascii_entities.scene.toml <work>/asciiweb .

# 2. the player and the bundle together, as play.chaps.dev has them (trunk, web-release)
flint export web <work>/asciiweb --out <work>/site      # or --player <a trunk dist> to skip the build

# 3. every scene at DPR 1 and 2, the canvas 1920x1080 device pixels at both
node tools/web-perf/run.mjs <work>/site games/asciiweb/asciiweb.flintpack \
  ascii_none,ascii_entities,ascii_full --dpr 1,2 --passes 'Style mask,ASCII' --out <work>/out
```

`<work>/out/table.md` is then `docs/design/web.md`'s browser table for the
stylize passes ("Stylize passes (#270)"). On 2026-11-12 (RTX 4070 laptop,
Chrome 157) it read:

| page | canvas | scene | Style mask ms | ASCII ms | GPU frame ms | bind groups a frame | passes a frame | frame p50 ms |
|---|---|---|---|---|---|---|---|---|
| DPR 1, ascii_none | 1920x1080 | 1920x1080 |  |  | 4.19--4.59 | 0.0 | 17.0 | 16.7 |
| DPR 1, ascii_entities | 1920x1080 | 1920x1080 | 0.04 | 0.34--0.41 | 4.91--5.30 | 0.0 | 26.0 | 16.7 |
| DPR 1, ascii_full | 1920x1080 | 1920x1080 | 0.02--0.04 | 0.52--0.92 | 3.22--4.85 | 0.0 | 26.0 | 16.7 |
| DPR 2, ascii_none | 1920x1080 | 1440x810 |  |  | 3.87--3.88 | 0.0 | 18.0 | 16.7 |
| DPR 2, ascii_entities | 1920x1080 | 1440x810 | 0.03--0.04 | 0.69--0.70 | 4.41--4.42 | 0.0 | 27.0 | 16.7 |
| DPR 2, ascii_full | 1920x1080 | 1440x810 | 0.02--0.69 | 0.49--0.90 | 2.56--4.44 | 0.0 | 27.0 | 16.7 |

A second run read per-entity ASCII at DPR 1 as 0.54--0.60: another job had
the GPU. Run it more than once.

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
| `--port N` | `9381` | Chrome's devtools port; pick another to run two at once |
| `--chrome <path>` | the usual install, or `CHROME` | |

Each page's JSON keeps the readout's text and its copyable line at every read
(`reads[].text`, `reads[].line`), the line parsed into numbers
(`reads[].parsed`, `perfline.mjs`), what the page saw (DPR, viewport, canvas,
user agent, whether the adapter has `timestamp-query`) and
`window.flintGpuCalls`' totals. A page whose gate never lifts is recorded
with its status line as `error`, and the run exits 1.

**What it needs from the machine.** Chrome draws WebGPU only headed, and gives
no frames to a window that is off screen or covered, so the window opens at
0,0 with the throttling flags off (and `CalculateNativeWinOcclusion` disabled):
leave it be while it runs. Other jobs on the GPU move the numbers; run twice
and look at a pass the change doesn't touch (SSAO, Bloom) to spot a contended
run. Tests for the line parser: `node --test tools/web-perf/perfline.test.mjs`.
