# Flint on the Web

Status: M0 spike (#95), M2 platform shims (#100) and async startup (#99)
done, epic #94. The browser entry runs the real `PlayerApp`: it fetches a
scene, opens the WebGPU device without blocking, and draws the scene to a
page canvas that follows the window's size. Files the scene points at
(models, textures, scripts, audio) go through the vfs (#97), which stays
empty on the web until game bundles (#98) fill it, so the web draws scenes
made of the engine's own geometry: boxes, the procedural sky, the Gerstner
ocean.
Sound (#101) plays behind a Begin gate on the shim page; the player is
silent on the web until its page calls `start_audio()` from a gesture
([Sound](#sound)).

## Try it

```bash
rustup target add wasm32-unknown-unknown
cargo check --target wasm32-unknown-unknown -p flint-player   # the whole player
cd crates/flint-player
trunk serve                 # http://127.0.0.1:8080, the player on the First Light scene
trunk serve shims.html      # the shim check (#100): sky clear, time, pad, rebinds;
                            #   sound behind a Begin click (#101)
trunk build --release       # dist/, with wasm-opt -Oz (trunk fetches binaryen)
```

CI keeps this compiling (#96). The `wasm32 check` job in
`.github/workflows/ci.yml` runs `scripts/wasm-check.sh` on every PR: a wasm32
clippy pass over `flint-player` and the workspace crates it links, with every
lint off but `disallowed_methods`, configured in
`scripts/wasm-clippy/clippy.toml` to deny `Instant::now`, `SystemTime::now`
and their `elapsed()`. Those are the calls that compile but panic. #100
moved the 32 sites that predated the check to `web-time` and removed
their allows, so a std time call in a runtime crate now fails the check.

The page is `crates/flint-player/index.html` and builds the `flint-web` bin
(`src/bin/flint_web.rs`), the web twin of `main.rs`. Its arguments come from
the query string instead of clap:

| Query | Desktop flag | Meaning |
|---|---|---|
| `scene=<url>` | `<scene>` | scene TOML, fetched relative to the page. Default `scenes/web_first_light.scene.toml`, copied from `web/scenes/` by trunk |
| `msaa=1\|4` | `--msaa` | scene-pass MSAA |
| `width=&height=` | `--width --height` | fix the canvas to that many CSS pixels; otherwise it fills the page |

The engine schemas are compiled in (`build.rs` embeds `schemas/components`
and `schemas/archetypes`), since a browser can't list a directory.

## Async startup (#99)

A desktop makes the window and then blocks on `RenderContext::new`. A
browser can't block its main thread, so on wasm32:

1. `flint_web.rs` fetches and parses the scene, builds `PlayerApp`, gives it
   an `EventLoopProxy` (`set_event_proxy`) and starts winit with `spawn_app`,
   which returns at once.
2. `resumed` → `initialize` makes the window on the page's `<canvas id="flint">`
   (`web_startup::canvas_attributes`; a page without one gets a canvas
   appended) and starts `RenderContext::new` with `spawn_local`.
3. The future parks the context in a shared slot and wakes the loop with a
   user event, or a redraw if no proxy was given.
4. `user_event` → `take_ready_render_context` runs `finish_initialize`, the
   same code a desktop runs after `block_on`, then sizes the surface to the
   canvas as it is now.

Until step 4 the player is *not ready*: `render_context` is `None`, and
`window_event` drops every event, so nothing ticks, draws or reads input
against a missing device. `resumed` starts only when there is no window yet,
so a second `resumed` while the device is in flight doesn't start twice.
Desktop and Android keep `pollster::block_on` and `run_app`.

The canvas's CSS size decides the surface size: winit's `ResizeObserver`
sends `Resized` with the CSS size times `devicePixelRatio`, and the player
reconfigures the surface. Measured in Chrome at DPR 1.5: a 1266x650 page
gave a 1899x975 surface, 886x470 gave 1329x705, 1386x710 gave 2079x1065,
with no WebGPU errors.

## WGSL that only the browser rejects

naga, which compiles WGSL on the desktop, does not check WGSL's uniformity
rules. Chrome's Tint does, and an invalid pipeline invalidates the whole
frame's command buffer, so one bad shader leaves the canvas black. Fixed in
#99:

- `ocean_shader.wgsl`: `fwidth` inside the rain-ring cell loop. The
  derivative is taken once before the branch.
- `ssao_shader.wgsl`: `textureSample` inside the kernel loop. The pass reads
  unmipped targets, so it uses `textureSampleLevel(.., 0.0)`.
- `grass_render.wgsl`: `textureSampleCompare` inside the shadow-bounds
  branch, now `textureSampleCompareLevel`.
- WebGPU forbids depth bias on line topologies. The overlay, skeleton and
  skinned-wireframe line pipelines draw unbiased on the web
  (`pipeline::line_depth_bias`).

Chrome gives a `Bgra8Unorm` surface, which is not sRGB, and the renderer
picks the first format when no sRGB one is offered. The web frame comes out
darker and more saturated than the desktop's. That still needs fixing,
either with an sRGB view format or an encode in the composite pass.

## What failed to compile, crate by crate

Five errors in all, against the seven crates the issue was worried about:

| Crate | Result on wasm32 | Fix in this branch |
|---|---|---|
| `getrandom` 0.3 (via ahash: egui, hecs, rhai) | refuses to build: "wasm32-unknown-unknown targets are not supported by default" | `wasm_js` feature on a wasm-only dep in `flint-player`, plus `--cfg getrandom_backend="wasm_js"` in `.cargo/config.toml` |
| `egui-winit` → `arboard` | no web clipboard backend (8 errors) | `egui-winit` with `default-features = false` on wasm only. This also drops `links`/`webbrowser` |
| `kira` / `cpal` | builds. `StaticSoundData::from_file` is not there on wasm (flint-audio once, flint-music twice) | `static_sound_from_file` helper: `from_file` on native, `std::fs::read` + `from_cursor` on web. Since #97 both targets read through the vfs and decode with `from_cursor`. (The spike thought cpal had the null host here. It didn't: kira turns on cpal's `wasm-bindgen` feature for wasm32 itself. See [Sound](#sound).) |
| `rhai` | builds. `module_resolvers::FileModuleResolver` is not there | `ScriptEngine::set_module_base` skipped the resolver on wasm. Since #97 it installs `VfsModuleResolver` (flint-script) on every target, so `import` resolves through the vfs |
| `wgpu` 23 | builds (WebGPU backend by default, no naga or wgpu-core). `Instance::enumerate_adapters` is not there | `gpu_select` treats the adapter list as empty on wasm. `--gpu` / `WGPU_ADAPTER_NAME` fall back to `request_adapter` |
| `rapier3d` / `parry3d` | builds clean | none |
| `gilrs` (xinput-only features, ADR 0011) | builds clean. gilrs-core picks its web Gamepad backend by target | none needed to compile. See "Shims landed (#100)" for features |
| `gltf`, `image`, `toml`, `hecs`, `winit` 0.30 | build clean | none |

`cargo tree` lists 390 distinct crates in the wasm graph.

## Compiles, but will fail at run time

These are what M1 and M2 owe (`#97` vfs, `#98` async startup, the shim
tickets). Counts are for the runtime crates the player links:

- **`std::fs`** (done in #97: every runtime read now goes through `flint_core::vfs`; the web player installs a `MemorySource` filled from its bundle, which #98 fetches) returns `Unsupported` on wasm32-unknown-unknown. LLVM can
  *prove* that, so it deletes everything behind a file read. The real
  `flint-player` bin comes out at 2.4 MB with 4 imports because `load_scene`
  always fails and the whole engine after it is dead code. Any size number
  taken from the stock bin is wrong until the vfs lands.
- **`pollster::block_on`**: done in #99, see Async startup above.
- **Audio:** done in #101, see [Sound](#sound).

### Shims landed (#100)

- **Time.** `std::time::Instant::now` and `SystemTime::now` panic ("time not
  implemented on this platform"). Every runtime use now goes through
  `web-time` (already in the graph through egui and rhai): `GameClock`,
  input timestamps, the save debounce, frame pacing, the music clock bridge
  and session, procgen's cache, queue and random seed, input capture, and
  rhai's `random`. On native `web_time` re-exports std's types, so the
  signatures callers see are the same. The unused `instant` workspace dep is
  gone. Tests and native-only tools (flint-cli, the viewer) keep std.
- **Gamepads.** gilrs-core picks its backend by target: XInput on Windows
  through the `xinput` feature (ADR 0011, unchanged), the browser Gamepad API
  on wasm32. The feature has to stay on for every target, because
  gilrs-core 0.6.7's "needs xinput or wgi" `compile_error!` isn't gated to
  Windows. It does nothing elsewhere, since rusty-xinput is a Windows-only
  dep. The direct `gilrs-core` dependency (the rumble motor path, ADR 0025)
  is native only.
- **Threads.** `flint-input-capture`'s 1 kHz capture loop, `measure_granularity`
  and the rumble spike compile out on wasm. `spawn_with_rumble` returns `Err`
  there, which `attach_session_input` already treats as "play without
  input". A music session on the web has no high-rate capture yet. The
  player's ordinary once-a-frame gilrs poll still drives `InputState`.
- **Settings.** `flint_player::user_files` holds the files the player writes
  for the person playing: ordinary files under `dirs::config_dir()` on the
  desktop, and `localStorage` keys on the web (`flint:/config/flint/input_<game_id>.toml`).
  The input override's resolve, read and write go through it, so a rebind
  survives a reload. `dirs` is a native-only dependency now.
- **Desktop-only tools.** `notify`, `rfd` and `ureq` aren't in the wasm
  graph at all: `cargo tree --target wasm32-unknown-unknown -p flint-player
  -i <crate>` finds none of them. Only flint-cli and the viewer use them.
  The remaining `std::process::id` calls in player crates are all in tests.

The shim page (`shims.html`, bin `flint-web-shims`) exercises all of these: the day runs on
`web_time::Instant`, the left stick scrubs it, `pause_day` (Space or the
pad's South button) holds it through `InputState`, and R followed by a key
or pad button rebinds `pause_day` and saves the rebind through `user_files`.

Over its sky sits the Begin gate (#101). Float's sea and one of its songs
are fetched from its GitHub repo and decoded, and the click opens the sound
(query parameters are listed at the top of `flint_web_shims.rs`: `?music=`,
`?buffer=`, `?audio=`, `?begin=auto`, `?stall=`). The corner counts late
buffers, and the console prints a steadiness line every 10 s.

## Sound

#101. A browser gives no sound before a user gesture: an `AudioContext`
made earlier is kept suspended, and Chrome warns about it. So on wasm32
`AudioEngine::new()` stays silent, and the shim page's **Begin** gate opens the
stream from inside the click handler:

- `AudioEngine::silent()` / `start()`. On the desktop `new()` is `silent()`
  then `start()`, as before. In the browser, `start()` is called from the
  gesture. A silent engine still loads and caches sounds, so decoding
  happens behind the gate. The filter cutoff and bus volumes set while
  silent are applied when it starts. If the device fails, the engine stays
  silent and `start()` can be called again (the `Option` gotcha still
  holds).
- `AudioSystem::start(world)` picks up what `initialize` skipped while
  silent: the listener, the trigger rules and every `audio_source` already
  in the world. Looping sources therefore begin from the click.
  `PlayerApp::start_audio()` also starts a scene's `music_session` that was
  skipped for want of a device. The web player entry (#98) should call it
  from its gate.
- `flint_audio::web::WebAudioBackend` replaces kira's `CpalBackend` on
  wasm32 (`engine::AudioBackend` picks the backend by target). It is the same
  cpal WebAudio stream, but it keeps cpal's timestamps. Each buffer's
  **slack** is how far ahead of the audio clock it was scheduled.
  `flint_audio::meter::BufferMeter` logs every late buffer (negative slack,
  heard as a gap), plus a summary every 10 s: buffers, late, slack min/mean,
  mix time and the longest wait between requests. It also logs the
  `AudioContext`'s state changes and device errors. `AudioEngine::buffer_stats()`
  returns the totals, which the page shows in its corner.

cpal's WebAudio host renders on the **main thread**: each buffer's
`onended` mixes the next one, with two in flight. So a frame that holds the
thread longer than about one buffer (2048 frames, 46 ms at 44.1 kHz) makes
sound drop. `?buffer=4096` doubles that headroom, at the cost of latency.

Measured in headless Chrome 156 on this laptop, with Float's `ocean_loop.ogg`
and `music_day.ogg` fetched from its repo. The `?stall=N` knob holds the main
thread for N ms every 2 s:

| Run | Buffer | Buffers | Late | Worst slack |
|---|---|---|---|---|
| no stall, **10 min** (`music_dusk`) | 46 ms | 12962 | 0 | +19.2 ms |
| no stall, 70 s | 46 ms | 1514 | 0 | +36.6 ms |
| 30 ms stall, 150 s | 46 ms | 3241 | 0 | +19.2 ms |
| 60 ms stall | 46 ms | 646 | 5 | −21.4 ms |
| 100 ms stall | 46 ms | 1080 | 45 | −62.1 ms |
| 100 ms stall | 93 ms | 326 | 1 | −4.0 ms |
| 200 ms stall | 93 ms | 324 | 19 | −102.7 ms |

Counts are as of each run's last 10 s summary. At 46 ms buffers, a 100 ms
stall loses about one buffer per stall. A stall is covered while it ends
before the playing buffer does, so the edge sits near one buffer length,
where the model puts it.

Mixing Float's sea and one song costs about 0.27 ms a buffer, so the mix
isn't the hazard. Heavy frames are. Decoding happens before Begin:
`ocean_loop.ogg` (156 KB) took 28 ms, `music_day.ogg` (1.26 MB) 241 ms, and
`music_dusk.ogg` (1.43 MB) 100 ms in another run.
A song decoded mid-game would stall that long, so Float should decode its
four songs up front, or behind a scene change. If heavy frames drop sound
once Float's ocean renders, follow Soba's threaded AudioWorklet build. It
needs cross-origin isolation headers.

**Firefox 147**, headless, with autoplay allowed by profile prefs: 865
buffers in 40 s, 0 late, worst slack +25.0 ms. Firefox's headless mode
offers no WebGPU, so the sky failed to create its surface while the sound
played on. Sound doesn't depend on the canvas.

What the headless runs can't show: whether it *sounds* clean on speakers,
and Safari (no way to run it from Windows). Those need a person at a browser
(the issue's ten-minute listen).

## Size

Release profile, wasm-bindgen 0.2.108, binaryen `wasm-opt` version 123 `-Oz`,
gzip -9:

| Build | wasm-bindgen output | after `wasm-opt -Oz` | gzip -9 |
|---|---|---|---|
| `flint-web` spike (wgpu + winit only) | 792 KB | 408 KB | 164 KB |
| `flint-web-shims`, the #100 shim check (+ `InputState`, toml, gilrs) | | 857 KB | 329 KB |
| Whole engine (size probe, below) | 14.9 MB | 10.7 MB | 4.1 MB |
| `flint-web` running PlayerApp (#99, measured before the vfs merge; file-reading code was still dead) | | 9.4 MB | 3.5 MB |

The **size probe** is a throwaway bin, not committed. It builds a
`PlayerApp` from a `black_box` world and passes opaque bytes to
`rhai::Engine::eval`, `image::load_from_memory`, `toml::from_str` and
`AudioEngine::load_sound_from_bytes`, then runs the event loop. That keeps the
code the vfs will open alive. The glTF mesh conversion still sits behind
`gltf::import(path)`, so the probe somewhat *undercounts*.

Code bytes by crate (9.5 MB code section, from the name section; generic std
glue is credited to the first crate in its type arguments):

| Crate | MB | | Crate | MB |
|---|---|---|---|---|
| rhai | 2.04 | | zune_jpeg | 0.21 |
| flint_script | 0.64 | | epaint | 0.18 |
| egui | 0.55 | | toml | 0.17 |
| hashbrown | 0.45 | | flint_player | 0.17 |
| flint_procgen | 0.45 | | winit | 0.17 |
| parry3d | 0.43 | | exr | 0.15 |
| flint_render | 0.35 | | image | 0.15 |
| rapier3d | 0.29 | | image_webp + tiff | 0.23 |

Trims worth trying before a size budget is promised:

1. **`image` formats.** exr, tiff, webp and zune-jpeg come to about 0.6 MB of
   code. Games ship PNG (and maybe JPEG), so set `default-features = false`
   with `png`/`jpeg`.
2. **rhai** is a fifth of the code. Its `no_optimize`, `no_custom_syntax` and
   `only_i64` features, or a leaner `sync`, are the levers. Measure each one.
3. **flint_procgen** (0.45 MB) is linked for runtime spline/mesh gen. Check
   whether the web player needs all of it.
4. `opt-level = "z"`, `lto = "fat"`, `codegen-units = 1` and `panic = "abort"`
   in a `web-release` profile. Not tried here.

4.1 MB gzipped is in the range of other WebGPU game demos. Float's 10 MB of
data, most of it music, will cost more than the engine does.
