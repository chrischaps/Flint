# Where Flint's hitches come from (#1080, 2026-11-16)

Epic #1075 built the tools to see hitches: an always-on frame history and
detector (#1076), frame notes that say what the engine did on a frame (#1077),
the F2 frame-time graph (#1078) and `--hitch-log` with an exit report and gate
(#1079). This note points them at real games and records what they found.

**Verdict: four engine causes and one machine cause, each filed.** Ordered by
what a player would feel:

1. **A ~515 ms freeze when another process starts or stops using the GPU**
   (#1120). The swapchain acquire waits half a second. It happened in every
   scene, on both backends, in every present mode, in release and debug, 2 to
   17 times per two minutes while other agents were building and rendering on
   the machine, and none in 10.5 quiet minutes. It is the longest hitch in
   almost every run. It goes through the hybrid laptop's cross-GPU present path.
2. **The showcase game hitches about once a second** (#1119): its script sets
   two save keys every frame to the value they already have, so the 1 s
   debounce rewrites the save file every second, on the frame thread. Each
   write costs a median 8 ms and up to 70 ms (release), and 21 of 91 cost
   more than a 60 Hz frame.
3. **A look's first use stops the game for 1.3 s (Vulkan) to 1.7 s (DX12)**
   (#1121) while the ASCII style's 15 pipelines are built, one after another.
4. **The hot-reload poll** sometimes takes 1 to 35 ms on the frame thread
   (#1122), with no script changed and even with none watched. *Fixed by
   #1122:* every poll worked out the script search's `lib/` folders afresh,
   and that walk stats the disk (each level's directory key, the engine
   mount). Traced in the tavern (release, 30 s): that one step was 0.19 ms of
   the poll's 0.21 ms median and 116 of 1,796 polls took 1 ms or more (worst
   14.2 ms); worked out once when the search is set, the whole poll is
   0.018 ms median and 0.076 ms worst.
5. **Scene transitions** take 40 to 150 ms (release) and 105 to 320 ms (debug)
   in one frame, but the showcase hides them behind a 1.5 s fade. They count
   as hitches anyway (#1125, a tool issue).

Half of what the detector counts at 240 Hz isn't visible. Of 1,421 hitches
logged in the 19-run matrix, 704 were frames under 16.7 ms, mostly two missed
refreshes of 4.2 ms. Outside the showcase's save-heavy runs it is most of
them: 413 of 495. That is the rule #1109 already questions; the column
"≥16.7 ms" below is the count that matters.

## The machine

Lenovo Legion laptop (the `Legion` host): Intel Core Ultra 9 275HX, 31 GB, an
**NVIDIA GeForce RTX 5070 Laptop GPU** (driver 32.0.15.9207) that renders, an
**Intel Graphics** iGPU (32.0.101.8331) that drives the built-in **2560-wide
240 Hz** panel, and a Virtual Desktop Monitor adapter attached. Windows 11
build 26300. Every window was 1280×720, opened with `--no-activate --position
40,40` so the runs didn't take the keyboard. Other Claude sessions and Chris's
dashboard daemon (`signald`) were running on the machine throughout, as they
usually are.

Wgpu picked the RTX in every run but one (`WGPU_ADAPTER_NAME=Intel`, below).

## What was run

Each run is `flint-player <scene> --hitch-log <run>.jsonl --exit-after <s>`,
one at a time, the backend from `WGPU_BACKEND`. Builds came from this commit
(`77f14747`) with the default features: `cargo build -p flint-player` and
`--release`.

- **The Overgrown Ruins** (`showcase_game/levels/tavern_hub.scene.toml`, with
  `--save-dir` holding a save with 7 essence so every zone door is open). A
  scripted walk to the library door didn't trigger its interact (the patron's
  prompt showed; not chased), so for these runs a temporary driver entity (not
  committed) called `load_scene` 6 s after each scene's start: tavern hub to
  collapsed library and back, 29 transitions in 3 minutes, each behind the
  game's own 1.5 s fade.
- **The looks tour** (`flint-examples/looks.scene.toml`): `looks_tour.rhai` calls
  `set_look` every 3 s through graphite, ascii_noir, matrix, ink_wash,
  watercolor and none.
- **Woozy Grove** (`flint-examples/woozy_grove.scene.toml` with
  `flint-examples/woozy_grove_walk.txt`): living props, wooze, a walk.
- **The Phase 4 tavern** (`flint-examples/phase4_runtime.scene.toml`): NPCs, audio,
  physics, a calm scene.
- **The hitch metronome** (`flint-examples/hitch_metronome.scene.toml`) as the
  control: a 100 ms script stall every 3 s. 43 s of it logged 14 script
  hitches, exactly its stalls, each named `script stall.rhai:on_update`.

No game outside the checkout was run: Chris named none.

## The runs

"Hitches" leaves out the first 2 s (startup, counted apart). "≥16.7 ms" is the
hitches a 60 Hz player would see. "~500 ms acquire" is the frames of 400 to
900 ms that weren't a load or a style build; a temporary timer (below)
showed each of them waiting in the swapchain acquire. One exception is the
797 ms frame in `ruins-rel-v0-dx12`, which spent its time between frames (gap
3). Causes are the detector's own (`sync` and `render` are `<phase> +x ms` with
no note).

| Run | Build | Present | Backend | s | fps | Hitches | ≥16.7 ms | ~500 ms acquire | Worst ms | Causes (count) |
|---|---|---|---|---|---|---|---|---|---|---|
| `ruins-dbg-fifo-dx12` | debug | Fifo | DX12 | 180 | 132 | 144 | 99 | 11 | 518 | sync 63, transition 29, elsewhere 18, ~500 ms acquire 11, physics steps 9, render 8, hud 3, reload poll 3 |
| `ruins-dbg-fifo-vk` | debug | Fifo | Vulkan | 180 | 210 | 140 | 64 | 5 | 518 | sync 75, transition 29, elsewhere 9, physics steps 7, hud 7, reload poll 6, ~500 ms acquire 5, scripts 2 |
| `ruins-dbg-v0-dx12` | debug | VSYNC=0 | DX12 | 180 | 135 | 142 | 124 | 1 | 510 | sync 100, transition 29, reload poll 6, physics steps 5, hud 1, ~500 ms acquire 1 |
| `ruins-dbg-v0-vk` | debug | VSYNC=0 | Vulkan | 180 | 223 | 137 | 78 | 6 | 494 | sync 97, transition 29, ~500 ms acquire 6, physics steps 2, render 1, hud 1, reload poll 1 |
| `ruins-rel-fifo-dx12` | release | Fifo | DX12 | 180 | 238 | 65 | 52 | 0 | 67 | sync 30, transition 29, physics steps 5, reload poll 1 |
| `ruins-rel-fifo-vk` | release | Fifo | Vulkan | 180 | 238 | 47 | 47 | 0 | 102 | transition 29, sync 17, reload poll 1 |
| `ruins-rel-v0-dx12` | release | VSYNC=0 | DX12 | 180 | 612 | 128 | 90 | 3 (+1 gap) | 797 | sync 91, transition 29, ~500 ms acquire 3, gap 1, reload poll 2, physics steps 2 |
| `ruins-rel-v0-vk` | release | VSYNC=0 | Vulkan | 180 | 511 | 123 | 81 | 0 | 149 | sync 93, transition 29, reload poll 1 |
| `looks-dbg-fifo-vk` | debug | Fifo | Vulkan | 120 | 213 | 109 | 11 | 7 | 1320 | render 83, elsewhere 15, ~500 ms acquire 7, style build 3, sync 1 |
| `looks-rel-fifo-dx12` | release | Fifo | DX12 | 120 | 215 | 12 | 9 | 6 | 1727 | ~500 ms acquire 6, style build 3, render 2, physics steps 1 |
| `looks-rel-fifo-vk` | release | Fifo | Vulkan | 120 | 218 | 13 | 5 | 2 | 1284 | render 5, style build 3, ~500 ms acquire 2, physics steps 1, elsewhere 1, reload poll 1 |
| `looks-rel-v0-vk` | release | VSYNC=0 | Vulkan | 120 | 552 | 200 | 8 | 5 | 512 | render 182, sync 7, ~500 ms acquire 5, reload poll 5, style build 1 |
| `grove-dbg-fifo-vk` | debug | Fifo | Vulkan | 120 | 36 | 7 | 7 | 6 | 454 | ~500 ms acquire 6, physics steps 1 |
| `grove-rel-fifo-dx12` | release | Fifo | DX12 | 120 | 171 | 94 | 6 | 2 | 512 | elsewhere 44, sync 38, reload poll 8, ~500 ms acquire 2, physics steps 2 |
| `grove-rel-fifo-vk` | release | Fifo | Vulkan | 120 | 227 | 24 | 5 | 4 | 514 | sync 14, ~500 ms acquire 4, reload poll 3, elsewhere 2, physics steps 1 |
| `grove-rel-v0-vk` | release | VSYNC=0 | Vulkan | 120 | 224 | 10 | 6 | 3 | 506 | sync 5, ~500 ms acquire 3, reload poll 2 |
| `tavern4-dbg-fifo-vk` | debug | Fifo | Vulkan | 120 | 117 | 7 | 7 | 4 | 503 | ~500 ms acquire 4, sync 3 |
| `tavern4-rel-fifo-vk` | release | Fifo | Vulkan | 120 | 239 | 2 | 2 | 2 | 520 | ~500 ms acquire 2 |
| `metronome-rel-fifo-vk` | release | Fifo | Vulkan | 45 | 230 | 17 | 16 | 2 | 519 | script stall.rhai:on_update 14, ~500 ms acquire 2, render 1 |

The pacing experiments, on Woozy Grove (the run with the most `elsewhere`
hitches), run 25 minutes later:

| Run | Build | Present | Backend | s | fps | Hitches | ≥16.7 ms | ~500 ms acquire | Worst ms | Causes (count) |
|---|---|---|---|---|---|---|---|---|---|---|
| `grove2-rel-fifo-dx12` | release | Fifo | DX12 | 90 | 128 | 11 | 7 | 0 | 20 | sync 5, reload poll 3, elsewhere 3 |
| `grove2-rel-lat1-dx12` | release | Fifo, `FLINT_FRAME_LATENCY=1` | DX12 | 90 | 108 | 4 | 4 | 0 | 33 | reload poll 2, sync 2 |
| `grove2-rel-lat3-dx12` | release | Fifo, `FLINT_FRAME_LATENCY=3` | DX12 | 90 | 133 | 4 | 4 | 0 | 25 | sync 3, reload poll 1 |
| `grove2-rel-nopace-dx12` | release | Fifo, `FLINT_PACING=0` | DX12 | 90 | 134 | 6 | 4 | 0 | 24 | sync 3, reload poll 2, hud 1 |
| `grove2-rel-mailbox-dx12` | release | `FLINT_VSYNC=mailbox` | DX12 | 90 | 129 | 4 | 3 | 0 | 26 | sync 4 |
| `grove2-rel-fifo-vk` | release | Fifo | Vulkan | 90 | 181 | 69 | 3 | 0 | 28 | sync 37, elsewhere 25, reload poll 6, input 1 |
| `grove2-rel-mailbox-vk` | release | `FLINT_VSYNC=mailbox` | Vulkan | 90 | 184 | 3 | 2 | 0 | 22 | sync 2, input 1 |

## The causes

### 1. The swapchain acquire waits ~515 ms (#1120)

**Signature**: `render +480..520 ms`, no engine note, often two a quarter
second apart. In a slow debug build it reads `physics steps 2 fixed steps`
instead (#1124), and on DX12 it sometimes read `elsewhere, present 505 ms`
(the wait landed in `present` rather than the acquire).

**What it is**: a temporary timer around `context.surface.acquire()`,
`encode_frame_to`, the submit and `queue.present` (removed, never committed)
put all of it in `get_current_texture`: `acquire 517.8 ms`, `acquire 513.1 ms`
for the pair at t = 70.6 s in the Phase 4 tavern.

**What sets it off**: other GPU work on the machine, not anything in the
scene.
- On the wall clock the stalls clustered at the same seconds of each minute
  (around :10, :37 and :43) through the 45-minute matrix, whatever scene ran.
- The pacing runs, 10.5 minutes with nothing else rendering, had none.
- A second `flint-player` running 50 s beside a probed run was followed within
  0.8 s of its exit by three 500 ms acquires. Eight 3 s clients 8 s apart
  were each followed by a stall within 1.3 s of starting or exiting in 4 of 8
  cases.
- With the Intel iGPU rendering (`WGPU_ADAPTER_NAME=Intel`, the GPU wired to
  the panel) there was one in 150 s.
- Firing `Get-CimInstance Win32_VideoController`, `WmiMonitorID`,
  `Get-PnpDevice -Class Display`, `Get-Process` with paths, `nvidia-smi`,
  `git status` and `claude agents --json` at known moments set off none.

What fires on the minute wasn't pinned down. A process CPU sampler had
`signald`'s node process busy for about 1.5 s at :43 and :09 of each minute,
but it kept running through the quiet runs too, so it can't be the whole
story. The next step is Chris's: a run in discrete-GPU-only mode. On a
machine where agents build and render beside a game, this is the biggest
hitch there is, and it may be what he has been seeing.

### 2. The showcase rewrites its save file every second (#1119)

**Signature**: `sync +10..165 ms`, no note, 1.03 s apart, only in the
showcase game.

`game_controller.rhai` sets `zone_garden_unlocked` and `zone_library_unlocked`
to `true` every frame once the essence is in. `PersistentStore::set` marks
the store dirty whatever the value, so the debounce writes a temp file,
`sync_all`s it and renames it every second, on the frame thread in
`finish_frame`, which `Phase::Sync` covers. With `RUST_LOG=info` the
`persist: saved` lines come at the hitches' cadence. Guarding the two calls
took a debug run's 7 `sync` hitches in 12 s to 0. A temporary timer measured
91 release saves: median 8.0 ms, p90 36.5 ms, max 69.9 ms. In release on
Fifo it is 17 to 30 hitches per 3 minutes, 17 to 23 of them visible. Under `FLINT_VSYNC=0`, where
the median frame is 1.5 to 2 ms, 52 to 57.

### 3. A look's first use builds its pipelines on the frame (#1121)

**Signature**: `style build ascii, 1279.6 ms`, with every pipeline beneath it
as a nested note. The frame notes named all of this, so it needed no digging.

| Style (first `set_look`) | Vulkan release | DX12 release |
|---|---|---|
| ascii (15 pipelines) | 1280 ms | 1723 ms |
| kuwahara (3) | 44 ms | 78 ms |
| watercolor (2) | 85 ms | 133 ms |

### 4. The hot-reload poll (#1122)

**Signature**: `reload poll 16 scripts watched, 35.3 ms`. 42 polls noted
themselves across the runs, most of them 1 to 8 ms, the worst 35 ms (showcase,
release). In Woozy Grove, a poll with 0 scripts watched took 1.2 to 3.5 ms.

### 5. Scene transitions, behind a fade (#1125)

**Signature**: `scene transition collapsed_library.scene.toml, 224.7 ms` with
nested `procgen` and `script load` notes.

| Build | Transition ms (median / max) | Procgen inside | Named by no note |
|---|---|---|---|
| release | 40-49 / 146 | 9-10 / 22 | 26-33 / 133 |
| debug | 105-118 / 321 | 33-38 / 221 | 53-59 / 124 |

The first visit on a cold procgen cache spent 157 ms of 225 generating
textures. The showcase's `zone_hud.rhai` fades to black for 1.5 s and only
then completes the transition, so these frames are behind a black screen.
They aren't a hitch a player sees, but they are 29 of the 47 hitches the
detector counted in the release run. The unnamed half of each load belongs
to #1092 (load-time breakdown), where the table was added as a comment.

### Smaller things the runs showed

- **Two-refresh frames at 240 Hz** are most of every count (#1109): Woozy
  Grove on DX12 Fifo logged 94 hitches, 6 of them visible; the looks tour
  without vsync 200, 8 visible. The data went on #1109.
- **`elsewhere` hitches are Fifo's bursts.** Woozy Grove's 25 to 46 were all but
  two under 16.7 ms. Mailbox, `FLINT_FRAME_LATENCY=1` or `3` and `FLINT_PACING=0`
  each took them to 0 (DX12: 3 to 0; Vulkan mailbox: 25 to 0). None of the
  four mattered to anything visible, and none changed the 500 ms acquires,
  which were absent in that window anyway.
- **A GPU-heavy look reads as a burst of `render` hitches without vsync**:
  the tour's `ink_wash` takes a frame from 1.8 to 11 ms, so about 15 to 30
  frames of each pass count until the median catches up (182 of `looks-rel-v0-vk`'s
  200). It's a step in cost, not a hitch.
- **Living props' `sync`**: Woozy Grove's `sync +4 ms` hitches are its pose
  and procgen work, visible a handful of times a run at most.
- **The first frames of every run** carry an acquire stall at t ≈ 0.55 s on
  the RTX (startup, not gated).
- **Debug Woozy Grove runs at 36 fps.** That's a cost, not a hitch.

## What the tools couldn't tell

What #1075's tools left to a temporary timer, a process sampler or a guess.
Both sibling epics carry this list as a comment, the frame-debugger epic
(#1081) and the tooling epic (#1090); the two that are bugs in #1075's own
tools are filed (#1123, #1124).

1. **The acquire is inside `render`** (#1123): the biggest hitch on this
   machine was classed as the engine's own work. No log line separates
   acquire, encode, submit and present.
2. **Cause lines name notes that explain a fraction of the hitch** (#1124):
   `physics steps` for a 410 ms acquire, a 1.2 ms `reload poll` for a 9 ms
   `sync`.
3. **Time between frames has no phase.** A 797 ms frame spent 1.4 ms in the
   phases and 0.1 ms presenting. The rest was outside the frame (the event
   loop, the window). The detector says `elsewhere, present 0.1 ms`, which
   is true but doesn't say where.
4. **Costs under the threshold are invisible.** The save cost a median 8 ms
   every second, which the log only showed when a write crossed 2× the median
   plus 8 ms. Its distribution needed a timer. A per-frame dump of the history
   (`--frame-log` CSV, every frame's phases, notes and GPU ms) would have
   shown it, and the periodicity, at once.
5. **`sync` is a catch-all**: animation, the procgen queue, the renderer's
   sync and the save share it, and the save has no frame note (#1119 asks for
   one).
6. **No GPU time on the log line.** `Frame::gpu_ms` exists but isn't written,
   so "waited on the GPU" (ink_wash) can't be told from "waited on the
   compositor" (#1120) without a probe (#1123 asks for it).
7. **Nothing names the other process.** The acquire stall's trigger is outside
   Flint. Only a system-wide trace (PresentMon, GPUView, ETW) could tie it to
   a process. A how-to for those belongs with the sampling-profiler guide
   (#1101).
8. **Wall-clock timestamps.** The log's `wall_time` counts from process start,
   so lining stalls up across runs took the runner's start times. An absolute
   timestamp per line (or the start time in a header line) would make
   cross-run and cross-process correlation direct.

## How to repeat it

```bash
cargo build --release -p flint-player
# the showcase, with a save that opens every door
mkdir -p /tmp/ruins && printf 'essence_collected = 7\n' > /tmp/ruins/default.toml
(cd showcase_game && WGPU_BACKEND=vulkan ../target/release/flint-player levels/tavern_hub.scene.toml \
   --save-dir /tmp/ruins --hitch-log ruins.jsonl --exit-after 180)
WGPU_BACKEND=dx12 target/release/flint-player flint-examples/looks.scene.toml --hitch-log looks.jsonl --exit-after 120
FLINT_VSYNC=0 target/release/flint-player flint-examples/woozy_grove.scene.toml \
   --input-script flint-examples/woozy_grove_walk.txt --hitch-log grove.jsonl --exit-after 120
```
Then `[perf] hitches: ... causes: ...` at exit, and one JSON line per hitch
in the log (the book's Profiling guide, Finding a hitch).
