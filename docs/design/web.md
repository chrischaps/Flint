# Flint on the Web

Status: M0 spike (#95), async startup (#99), the vfs (#97), platform shims
(#100) and game bundles (#98) done, epic #94. The browser entry runs the
real `PlayerApp`: it fetches a game bundle with a progress bar, serves every
read from it through the vfs, opens the WebGPU device without blocking and
draws to a page canvas that follows the window's size. A bare page with no
bundle still plays the First Light scene, made of the engine's own geometry.

## Try it

```bash
rustup target add wasm32-unknown-unknown
cargo check --target wasm32-unknown-unknown -p flint-player   # the whole player
cd crates/flint-player
trunk serve                 # http://127.0.0.1:8080, the player on the First Light scene
trunk serve shims.html      # the shim check (#100): sky clear, time, pad, rebinds
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
| `bundle=<url>` | `--bundle` | a `.flintpack` from `flint bundle`, relative to the page. Its manifest names the scene and schemas; its late bundles are fetched beside it |
| `scene=<url>` | `<scene>` | with `bundle`: a scene inside it instead of the manifest's. Without: a scene TOML fetched relative to the page. Default `scenes/web_first_light.scene.toml`, copied from `web/scenes/` by trunk |
| `msaa=1\|4` | `--msaa` | scene-pass MSAA |
| `width=&height=` | `--width --height` | fix the canvas to that many CSS pixels; otherwise it fills the page |

The engine schemas are compiled in (`build.rs` embeds `schemas/components`
and `schemas/archetypes`), since a browser can't list a directory. A
bundle's own schema directories load over them.

## Game bundles (#98)

A browser has no files, so a game ships as one or two fetched blobs. `flint
bundle <game>` packs the game folder; `flint_core::bundle` is the format.

```bash
flint bundle ../Float                     # dist/float.flintpack, dist/float.audio.flintpack
flint bundle ../Float --list --dry-run    # every file, its size and its bundle; writes nothing
flint-player --bundle dist/float.flintpack   # play it on the desktop, every read from the bundle
# web: put both files next to index.html and open ?bundle=float.flintpack
```

**Format.** `FLINTPAK`, a `u32` format version, a `u32` manifest length, the
manifest as TOML, then every file's bytes back to back, uncompressed. The
manifest names the game, the start scene, the schema directories in load
order, the late bundles and each file's path, offset and size. Most of a
game is PNG, GLB and Ogg, compressed already; a host can still gzip the
whole file on the wire. `bundle::total_len` reads the real size from the
first bytes, so the progress bar is right even when Content-Length is the
gzipped size.

**What goes in.** Everything under the game root but the default excludes:
dot-folders, `target/`, `dist/`, `docs/`, `screenshots/`, `snapshots/`,
`renders/`, `reference/`, `scratch/`, `tmp/`, `logs/`, `tools/`, `src/`,
`engine/` (except `engine/schemas/`), Markdown, shell and Python scripts,
source art (`.blend`, `.psd`, `.kra`, `.xcf`) and Cargo files. A
`bundle.toml` in the game root adds to them:

```toml
name = "float"
scene = "scenes/float.scene.toml"
schemas = ["engine/schemas", "schemas"]   # the default
exclude = ["scenes/mixamo_spike.scene.toml", "scenes/snap_*.scene.toml"]
include = []          # wins over any exclude
[late]                # replaces the default
audio = ["**/*.ogg", "**/*.wav", "**/*.mp3", "**/*.flac"]
```

**Audio comes late.** By default every audio file goes to a second bundle the
player fetches once it is running. A browser won't start sound before the
first click anyway (#101), so nothing is lost by playing the first frames
silent. When a late bundle lands the web entry installs the grown source
(`vfs::install`), `vfs::generation()` moves, and the player runs
`load_audio_from_world` again: sounds already loaded are skipped. An
autoplay `audio_source` whose sound isn't loaded yet now waits unsynced
(`flint-audio/src/sync.rs`) instead of being marked done, so the ocean loop
starts when its file arrives. One-shot sounds a script fires before then are
lost, the same as with a missing file. `flint-player --bundle` adds the late
bundles three seconds in, so the desktop takes the same path.

**Checked before it's written.** A missing file should fail in the packer,
not as a pink texture deep in play. Before writing, the packer:

- loads every packed scene from the packed files, with the bundled schemas,
  expanding prefabs through the vfs, and fails on an archetype no schema
  defines;
- reads every string in scenes, prefabs, particle effects and chunks that
  looks like a file (a known extension after a stem), plus each
  `model.asset`, and fails if none of the places the runtime looks (beside
  the file, one level up, and their `models/`, `scripts/`, `audio/`,
  `textures/`, `sprites/` ... folders) holds it. A model may also be a
  `<name>.procgen.toml` the player builds;
- does the same for string literals in Rhai scripts. A name a script builds
  at run time (`"splash_" + n + ".ogg"`) can't be checked; the literal
  `".ogg"` has no stem and is skipped.

Packing Float on the first try failed this way: `scenes/mixamo_spike.scene.toml`
names `mixamo_body.glb`, which isn't in the repo. Float's `bundle.toml`
leaves that spike scene out, with the `snap_` scenes kept for reference
renders.

**Float**, packed with the `bundle.toml` above (Float at `7b37e1e`, its
engine subtree's schemas):

| Bundle | Files | Size | gzip -9 |
|---|---|---|---|
| `float.flintpack` (scenes, schemas, scripts, models, textures, sprites) | 92 | 3.66 MB | 1.59 MB |
| `float.audio.flintpack` (49 Ogg files, 5.3 MB of it the four music beds) | 49 | 7.11 MB | 6.96 MB |

`models/body.glb` is 2.45 MB of the main bundle on its own.

**In Chrome** (release build, `?bundle=float.flintpack`, network throttled to
1.5 MB/s over CDP): the 14.2 MB wasm took about 10 s, then the status line
counted `float.flintpack 0.3 / 3.7 MB` up to 3.7 over 2.3 s. The scene loaded
from the bundle (44 entities) and drew Float's raft, body and ocean while
`audio 0.2 / 7.1 MB` filled over the next 4.8 s, and the bar faded on
`late bundle audio: 49 files`. The frame then stopped on a `std::time`
panic right after startup ("time not implemented on this platform"): this
was measured before #100 moved those sites to `web-time`. With every engine read now live, the release
wasm is 14.2 MB after `wasm-opt -Oz`, 5.1 MB gzipped (it was 9.4 / 3.5 MB
while the vfs left file-reading code dead).

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
| `kira` / `cpal` | builds. `StaticSoundData::from_file` is not there on wasm (flint-audio once, flint-music twice) | `static_sound_from_file` helper: `from_file` on native, `std::fs::read` + `from_cursor` on web. Since #97 both targets read through the vfs and decode with `from_cursor`. cpal builds without its `wasm-bindgen` feature, so it gets the null host and there is no sound yet |
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
- **Audio:** `AudioManager::new()` already sits behind an `Option`, so the
  null cpal host degrades to silence. Real sound needs cpal's
  `wasm-bindgen` feature and a click to start the `AudioContext`.

## Size

Release profile, wasm-bindgen 0.2.108, binaryen `wasm-opt` version 123 `-Oz`,
gzip -9:

| Build | wasm-bindgen output | after `wasm-opt -Oz` | gzip -9 |
|---|---|---|---|
| `flint-web` spike (wgpu + winit only) | 792 KB | 408 KB | 164 KB |
| `flint-web-shims`, the #100 shim check (+ `InputState`, toml, gilrs) | | 857 KB | 329 KB |
| Whole engine (size probe, below) | 14.9 MB | 10.7 MB | 4.1 MB |
| `flint-web` running PlayerApp (#99, measured before the vfs merge; file-reading code was still dead) | | 9.4 MB | 3.5 MB |
| `flint-web` reading game bundles (#98; every engine read live) | | 14.2 MB | 5.1 MB |

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
