# Flint on the Web

Status: M0 spike done (#95, epic #94). `flint-player` and all its
dependencies compile for `wasm32-unknown-unknown`, and a trunk page clears a
WebGPU canvas in Chrome with no console errors. Sound works behind a Begin
gate (#101, [Sound](#sound)). Nothing here runs a scene yet.

## Try it

```bash
rustup target add wasm32-unknown-unknown
cargo check --target wasm32-unknown-unknown -p flint-player   # the whole player
cd crates/flint-player
trunk serve                 # http://127.0.0.1:8080, the sky-clear spike
trunk build --release       # dist/, with wasm-opt -Oz (trunk fetches binaryen)
```

CI keeps this compiling (#96). The `wasm32 check` job in
`.github/workflows/ci.yml` runs `scripts/wasm-check.sh` on every PR: a wasm32
clippy pass over `flint-player` and the workspace crates it links, with every
lint off but `disallowed_methods`, configured in
`scripts/wasm-clippy/clippy.toml` to deny `Instant::now`, `SystemTime::now`
and their `elapsed()`. Those are the calls that compile but panic. The 32
sites that predate the check are marked with an allow naming #100, so
`grep -rn disallowed_methods crates` is that ticket's list.

The page is `crates/flint-player/index.html` and builds the `flint-web` bin
(`src/bin/flint_web.rs`). That bin makes a canvas through winit, opens a wgpu
device on `Backends::BROWSER_WEBGPU` and clears it every frame. The colour
eases through a 60-second day (night, dawn, morning, noon, afternoon, dusk).
Chrome hands it a `Bgra8Unorm` surface, which is not sRGB, so the sRGB sky
stops are cleared as they are.

Over the sky sits the Begin gate. Float's sea and one of its songs are
fetched from its GitHub repo and decoded, and the click opens the sound
(query parameters are listed at the top of `flint_web.rs`: `?music=`,
`?buffer=`, `?audio=`, `?begin=auto`, `?stall=`). The corner counts late
buffers, and the console prints a steadiness line every 10 s.

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
| `gilrs` (xinput-only features, ADR 0011) | builds clean. gilrs-core picks its web Gamepad backend by target | none needed to compile. M2 still owes per-target features |
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
- **`Instant::now`** panics ("time not implemented on this platform"). 25
  calls in 11 files, including `GameClock`. Move to `web-time`.
- **`pollster::block_on`** at `player_app/init.rs:309` (the render context).
  A browser can't block on a promise, so it needs `spawn_local` and winit's `spawn_app`, as the spike
  does.
- **`std::thread`**: `flint-input-capture` spawns its 1 kHz capture thread
  and the rumble path sleeps. Both panic on wasm. Gate them, or use the
  gamepad API's polling on the main thread.
- **`dirs::config_dir`** returns `None`, so user input overrides go to
  `localStorage` instead.
- **Audio:** done in #101, see [Sound](#sound).

## Sound

#101. A browser gives no sound before a user gesture: an `AudioContext`
made earlier is kept suspended, and Chrome warns about it. So on wasm32
`AudioEngine::new()` stays silent, and the page's **Begin** gate opens the
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
| Whole engine (size probe, below) | 14.9 MB | 10.7 MB | 4.1 MB |

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
