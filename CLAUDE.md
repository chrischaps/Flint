# CLAUDE.md

This file provides guidance to Claude Code when working with this repository.

## Project Overview

Flint is a CLI-first, AI-agent-optimized 3D game engine written in Rust. The primary interface is CLI and code; visual tools validate results rather than create them. Phases 1-5 complete (ECS, constraints, PBR rendering, scripting/physics/audio/animation, AI asset generation). Game projects live in their own repositories with the engine as a git subtree.

## Build & Development Commands

```bash
cargo build                    # Debug build
cargo build --release          # Release build
cargo run --bin flint -- <cmd> # Run CLI
cargo run --bin flint -- play demo/phase4_runtime.scene.toml  # Play a scene
cargo run --bin flint-player -- demo/phase4_runtime.scene.toml --schemas schemas  # Standalone player
cargo test                     # Run all tests
cargo test -p flint-core       # Test a single crate
cargo clippy                   # Lint
cargo fmt --check              # Check formatting
```

## Development Workflow

Flint uses an **edit -> validate -> play** loop:

1. **`flint render`** -- Headless scene-to-PNG snapshot. Primary validation tool for AI agents.
   ```bash
   flint render levels/demo.scene.toml --output test.png --schemas schemas --width 1280 --height 720
   flint render levels/demo.scene.toml --output test.png --schemas schemas \
     --distance 20 --pitch 30 --yaw 45 --target 0,1,0 --no-grid
   # Debug/post-processing flags:
   #   --debug-mode wireframe|normals|depth|uv|unlit|metalrough
   #   --wireframe-overlay    --show-normals    --no-tonemapping
   #   --no-shadows           --shadow-resolution 2048
   #   --no-postprocess       --bloom-intensity 0.08  --bloom-threshold 1.0
   #   --exposure 1.5         --ssao-radius 0.5       --ssao-intensity 1.0
   #   --ssao-samples 64      (1-64; SSAO is the heaviest per-pixel pass — 16 is ~4x cheaper)
   #   --fog-density 0.02     --fog-color 0.7,0.75,0.82  --fog-height-falloff 0.1
   #   --dither-intensity 0.03   --desaturate 0.85
   #   --dof 0.6  --dof-focus 10  --dof-range 5
   #   --volumetric-density 1.0  --volumetric-samples 32
   #   --kuwahara-radius 4       --kuwahara-sharpness 8.0
   #   --kuwahara-hardness 8.0   --kuwahara-anisotropy 1.0
   #   --oren-nayar 0.7          --sheen-strength 0.15  --sheen-color 1,0.9,0.8
   #   --grade-lift 0.03,0.02,0.015  --grade-gamma 1,1,1  --grade-gain 1.04,1,0.94
   #   --film-grain 0.03         --grain-time 0  --fxaa
   #   --particle-time 2.0    (step emitters/effects deterministically before
   #                           capture; default none = no particles. ADR 0068)
   #   --msaa 4               (1|4; default 1 keeps pixel gates single-sample.
   #                           Also on flint-player. ADR 0058)
   # Note: --shadow-resolution defaults to 2048 (the renderer's construction
   # default) and is a real control since the texel-size upload (ADR 0049).
   #   --render-mode 1 --mode-mix 1.0 --mode-params 3,0,6,0
   #     (modes: 1=matrix 2=blood 3=drunk 4=tron [reality tears; params =
   #      mask scale, mask style 0 fbm/1 iris, rate, spare] 5=underwater
   #      [params = signed eye depth m, sea energy 0-1, daylight 0-1,
   #      biolum 0-1; masks by per-pixel waterline, not the tear mask])
   ```

2. **`flint edit <file>`** -- Unified interactive editor. Auto-detects file type and opens the right tool:
   ```bash
   flint edit levels/demo.scene.toml              # Scene viewer (hot-reload)
   flint edit levels/demo.scene.toml --spline      # Spline/track editor
   flint edit models/character.glb                 # Model previewer (orbit camera)
   flint edit models/character.glb --watch         # Model previewer with file watching
   flint edit specs/oak_tree.procgen.toml          # Procgen previewer (mesh/texture)
   flint edit specs/stone_wall.procgen.toml        # Texture pipeline editor (if pipeline pattern)
   flint edit terrain.terrain.toml                 # Terrain editor
   flint edit fx/fire.particles.toml               # Particle effect editor (curves, forces, bursts, scrub timeline;
                                                   #   --preset fire|smoke|sparks|rain creates a missing file;
                                                   #   --render out.png --anim-time 1.5 = deterministic snapshot)
   ```
   Supported extensions: `.scene.toml`, `.chunk.toml`, `.procgen.toml`, `.terrain.toml`, `.particles.toml`, `.glb`, `.gltf`.
   Common flags: `--width`, `--height`, `--no-grid`, `--watch`, `--seed`, `--no-inspector`, `--auto-orbit`.
   Scene viewer applies the scene's `[post_process]` block on load. **F4 opens the Rendering & Effects menu** (ADR 0053) — every render/post toggle plus its non-binary parameters (SSAO radius/samples, DoF, bloom, fog, grade/grain/FXAA, kuwahara, render modes, shadows + resolution, lighting levers, FOV, debug shading mode), an authored-vs-viewer-default post toggle, DoF-follow (focus plane tracks the last selected entity; values are plain view distances since the ADR 0055 depth fix), and a Particles section (the viewer simulates `particle_emitter` / `particle_effect` entities live; ADR 0068). WASD orbit, Q/E zoom — while an entity is selected, W/E/R switch gizmo mode instead of orbiting. It also seeds the orbit camera from the scene's authored `[camera]` block (Space returns to that framing; ADR 0051 in game repos using this pattern), and supports `--auto-orbit` turntable with O toggle and [ / ] speed like the model previewer.
   Model flags: `--clip <name>`, `--layer clip[:weight[:mask[:mode]]]` (repeatable), `--sequence <file.sequence.toml>`, `--sequence-loop`, `--anim-time <s>` (with `--render`; replays a sequence deterministically), `--anim-speed <f32>`, `--render <path.png>`, `--no-animate`, `--distance`, `--yaw`, `--pitch`, `--target`, `--fov`.
   A `*.sequence.toml` lists timestamped animator events (`blend`/`layer`/`speed`/`cue`); the previewer gets a Sequence panel (seek slider + event markers, R = restart), the player auto-loads them from `animations/` and scripts use `play_sequence` / `on_sequence_cue`. Layer weights can ramp via `fade_target`/`fade_duration` (`fade_anim_layer`).
   The model previewer has a Layers stack (clip/weight/Add-Over/mask/solo-mute per row) and a "Skeleton colour" combo that paints the armature by last writer / layer weight / mask / keyed joints.
   Scene flags: `--spline` (opens spline/track editor).
   Old commands (`serve`, `preview`, `gen-preview`, `tex-edit`, `terrain-edit`) still work as hidden aliases.

3. **`flint gen`** -- Run a procedural generation spec to produce meshes (GLB) or textures (PNG).
   ```bash
   flint gen specs/oak_tree.procgen.toml -o tree.glb
   flint gen specs/stone_wall.procgen.toml -o wall.png
   flint gen specs/oak_tree.procgen.toml --dry-run
   flint gen specs/oak_tree.procgen.toml --seed 42 -o tree.glb
   ```

4. **`flint play`** -- Full game runtime with physics, scripting, audio, and input.

**For AI agents:** After visual changes, always run `flint render` to verify before moving on. The render command loads models (searches `scene_dir/models/` then `../models/`), generates geometry, and computes the full transform hierarchy.

## Architecture

26-crate Cargo workspace (plus `tools/arch-analyzer`, a syn-based dependency/metrics extractor feeding the `tools/arch-viewer` web graph; neither is a default member):

```
flint-cli           CLI binary (clap). Commands: init, entity, scene, query, schema, edit, play, validate, asset, render, gen, prefab,
  │                 validate-suite, play-suite, calibrate, play-chart, replay-chart, render-suite, spike-rumble, bundle
  ├── flint-asset-gen AI asset generation: pluggable providers (Flux, Meshy, ElevenLabs, Mock)
  ├── flint-procgen   Procedural generation: Generator trait, registry, specs, GLB export
  └── flint-procgen-ai AI-assisted procgen: ProcGenAgent trait, spec creation/refinement
flint-android       Android entry point (NativeActivity, APK asset extraction)
flint-player        Standalone player (game loop, physics, audio, animation, scripting, egui HUD)
  ├── flint-music    Rhythm sessions: suite manifests, charts, tempo maps, ladder/seam/reintegration, replay, offline render
  ├── flint-input-capture 1 kHz gamepad capture thread (gilrs XInput) stamped against the audio clock; verb maps
  ├── flint-debug-ui DebugPanel trait + F3/F4 panel roster (Rendering & Effects, ocean, sky, camera, grass, particles...); shared `widgets` (drag helpers, CurveEditor, GradientEditor); optional via debug-hud
  ├── flint-script   Rhai scripting with hot-reload
  ├── flint-particles Particle sim: effect assets, curves, forces, bursts, sub-emitters, deterministic stepping
  ├── flint-animation Property tween + skeletal animation
  ├── flint-audio    Kira spatial audio
  ├── flint-terrain  Heightmap terrain with splat-map blending
  ├── flint-physics  Rapier 3D (character controller, static/kinematic/dynamic bodies, impulse joints)
  ├── flint-runtime  GameClock, InputState, InputConfig, EventBus, GameStateMachine, PersistentStore
  ├── flint-viewer   egui inspector with transform gizmos, undo/redo, TOML write-back
  ├── flint-import   glTF model importer
  ├── flint-asset    Content-addressed asset catalog
  ├── flint-constraint Constraint evaluation + auto-fix
  ├── flint-query    PEG query language (pest parser)
  ├── flint-scene    TOML scene serialization/deserialization
  ├── flint-render   wgpu PBR renderer (winit 0.30, wgpu 23, HDR post-processing, bloom, SSAO, fog)
  ├── flint-ecs      hecs wrapper with stable EntityId mapping (BiMap)
  ├── flint-schema   Component/archetype schema system
  └── flint-core     EntityId, ContentHash, Transform, Vec3, Color, FlintError, vfs (read-only file layer)
```

## Key Design Principles

- **Dynamic components** stored as `toml::Value` -- archetypes defined at runtime in schema TOML files, not Rust types
- **Stable EntityIds** via atomic counter (never recycled), persisted across save/load
- **Scene format is TOML** -- `[scene]` metadata + `[entities.<name>]` sections; reload is full re-parse
- **Schemas in `schemas/`** -- `components/*.toml` for field types, `archetypes/*.toml` for bundles. Multiple `--schemas` paths; later overrides earlier (game overrides engine)
- **Scripting via Rhai** -- `script` component with `.rhai` files in `scripts/`; callbacks: `on_init`, `on_update`, `on_collision`, `on_trigger_enter/exit`, `on_action`, `on_interact`, `on_draw_ui`, `on_scene_exit/enter`, `on_animation_end`, `on_sequence_cue`, `on_event` (custom events from `fire_event_data`, the engine or `ExtensionCtx::fire_event`); `ScriptCommand` for deferred effects; `DrawCommand` for immediate-mode 2D UI
- **Particles** -- inline `particle_emitter` or reusable `particles/*.particles.toml` effects via `particle_effect` (multi-emitter, curves, forces, bursts, sub-emitters; ADR 0068). Deterministic: per-emitter seeds from entity *names*, fixed-step `simulate_to`, `flint render --particle-time`. One shared `ParticleInstance` type; every consumer uploads through `SceneRenderer::update_particles_from` after `ParticleSystem::pack`. Scripts: `play_effect`/`stop_effect`/`set_effect_param`. F3 Particles panel in the player; F4 Particles section in the viewer
- **All game UI is script-driven** -- no hardcoded HUD; `hud_controller` entity + `hud.rhai` pattern; engine provides draw primitives (`draw_text/rect/circle/line/sprite`), scripts compose UI
- **Game project pattern** -- games in own repos with engine as git subtree at `engine/`; `--schemas engine/schemas --schemas schemas` for layered overrides
- **Player extensions** (ADRs 0071-0075) -- a game binary embeds `PlayerApp` and installs `PlayerExtension`s (`add_extension`): `register_script_api` (native Rhai fns; same signature replaces Flint's), `pre_update(ExtensionCtx)` before scripts' `on_update` (world, clock, input, last frame's camera, `gpu` for `upload_mesh`/`remove_mesh`, `attach_mesh`, `fire_event` -> scripts' `on_event` same frame), `on_window_event` (only input events consumable) and `ui(egui)` drawn between script UI and the Foreground debug panels. `fonts(&mut FontDefinitions)` edits egui's fonts before each install (ADR 0081); `fonts/fonts.toml` `[egui] proportional/monospace` names egui's default faces, and every named family falls back to egui's fonts. With any extension installed, egui sees input first; a release follows its press. Runtime meshes are frustum-culled per draw by their AABBs (every game). Picking: `Camera::screen_to_ground` / Rhai `screen_to_ground`. Book: guides/player-extensions.md
- **Every runtime read goes through `flint_core::vfs`** (#97) -- `vfs::read` / `read_to_string` / `read_dir` / `exists` / `is_file` / `is_dir`, never `std::fs` or `Path::exists` in a runtime crate. The default source is the disk; the web player installs a `MemorySource` filled from a game bundle (`vfs::install`; `flint bundle <game>` packs one, `flint_core::bundle` is the format, `flint-player --bundle` plays one natively, audio arrives in a late bundle and `vfs::generation()` tells the player to load it; docs/design/web.md), and tests use `vfs::scoped` (this thread only). Decode images with `flint_render::open_image` (not `image::open`) and glTF through `flint_import::import_gltf` (not `gltf::import`), both of which read via the vfs. Writes (scene save, saves, replays) and hot-reload mtime polling stay on `std::fs`, natively; input overrides go through `flint_player::user_files` (OS config dir, or `localStorage` on the web, #100), and on the web so does the `persist_*` store (`save_<game_id>.toml`, #104). Tests and the CLI keep `std::fs`
- **Fixed-timestep physics** (1/60s) via accumulator; animation/particles run at variable rate
- **Physics joints** (ADR 0069) -- `joint` component (hinge/prismatic/spherical/fixed) on a dynamic entity, attached to `joint.parent` or the transform parent. Bodies are placed from `get_world_matrix` and dynamic poses write back as local `position` + `rotation_quat`; unit scale is assumed on the physics chain. TOML units are degrees/metres, converted to Rapier radians at sync. Scripts drive `motor_target` via `set_joint_target` and pose rest-quaternion nodes with `rotate_local` / `set_rotation_quat` (`set_rotation` still deletes the quaternion). Bench: `demo/joint_test.scene.toml`
- **Two-bone IK** (ADR 0070) -- `ik_two_bone` component on the second bone of a transform chain names `target`, `pole` and `tip` entities; `IkPass` (flint-animation) solves after scripts/physics/animation each frame and before the renderer reads world matrices, writing only the two bones' `rotation_quat` from their captured rest. Lengths come from the model's rest offsets. `flint render` runs it once
- **Post-processing** -- HDR pipeline (`Rgba16Float`) with bloom, SSAO, fog, volumetric (god rays), Kuwahara (anisotropic painterly filter), vignette; `[post_process]` TOML block; F4 opens the Rendering & Effects debug menu in the player and viewer (all toggles + parameters, ADR 0053); when active, PBR shaders output linear HDR
- **Scene transitions** -- `TransitionPhase` lifecycle with script-driven visuals; `PersistentStore` survives transitions; `GameStateMachine` pushdown automaton with per-system `SystemPolicy`
- **Input system** -- TOML-based `InputConfig` with layered loading (built-in -> game -> user overrides -> CLI); keyboard/mouse/gamepad/touch unified via `Binding` enum

## Critical Gotchas

- `toml::toml!` macro produces `Map<String, Value>` not `Value` -- needs `.into()` in tests
- wgpu v23: `Instance::new()` takes owned `InstanceDescriptor`; max 4 bind groups (0-3); `wgpu::Queue` is not `Clone` -- pass `&Queue` through, do not stash one in a struct
- wgpu v23 on DX12: the shader-visible sampler heap has 2048 slots and every bind group's samplers are copied into it, with no sharing, so samplers in a per-entity or per-material bind group cap how many can live ("Not enough memory left" in `create_bind_group`, DX12 only; test with `WGPU_BACKEND=dx12`). Keep samplers in groups bound once a frame: PBR materials sample with the light group's `material_sampler` (group 2, binding 4), and a material bind group holds none
- winit v0.30: `ApplicationHandler` trait with `run_app()`; `DeviceEvent::MouseMotion` for raw mouse delta
- Rapier v0.22: character controller types in `rapier3d::control`, NOT `rapier3d::prelude`
- Kira v0.11: `Decibels(f32)` and `PlaybackRate(f64)` are tuple structs; uses `glam` via `mint`
- `AudioManager::new()` / `Gilrs::new()` may fail in headless/CI -- wrap in `Option`
- Rhai v1.24 `sync`: `call_fn()` Scope visible only to direct callee, NOT sub-functions; entity IDs as `i64`; no implicit numeric coercion (`0.0` is NOT `0`); draw `_ex` functions take `layer` as int (`0`), not float (`0.0`)
- `ScriptCallContext` uses raw `*mut FlintWorld` -- only valid during `call_update()`/`process_events()` scope
- `on_update()` is parameterless -- use `delta_time()` API; `on_interact` checks `interactable.range` + `.enabled`
- `on_draw_ui()` ALWAYS runs even when scripts paused (pause menus need it)
- HDR format `Rgba16Float` -- all pipelines must use this when post-processing active; `resize_postprocess()` on window resize
- `render_draw_commands()` is a free function to avoid borrow conflicts; `load_pending_sprites()` before egui borrow
- Screen size for draw commands must be egui logical points, NOT physical pixels
- Skeletal animation: separate `SkinnedVertex`/`skinned_pipeline`; bone matrices in storage buffer (bind group 3)
- Particles: no depth write; one shared storage buffer indexed by `first_instance`; skip draw for 0 alive; `bytemuck::cast_slice` for zero-copy
- `flint-terrain` does NOT depend on `flint-render`; uses standard `Vertex` format so reuses shadow pipeline
- `flint-android` excluded from `default-members` -- use `cargo ndk -p flint-android`; min API 26. `cargo test --workspace` fails on `ndk-sys` on Windows; use plain `cargo test`
- Windows links a 1 MB main-thread stack; debug builds of flint-cli's clap parser overflow it -- `.cargo/config.toml` links with `/STACK:8388608`. Don't remove it, and don't grow `Commands` assuming stack is free.
- Scene entity `merge_component()` does field-level merge INTO archetype defaults, not full replacement
- `flint_core::toml_util` -- use `toml_f64`/`toml_f32`/`toml_vec3`/etc. instead of inline coercion patterns
- Never use `archetype = "furniture"` on non-visual entities -- furniture includes `bounds`, renders teal boxes
- Per-frame GPU allocations are the renderer's CPU cliff: `update_from_world` rebuilds the draw lists every frame, so anything it needs per entity must come from a cache (`draw_bind_cache` for transform/material binds keyed by entity + sub-mesh with a material hash, `box_mesh_cache` for procedural boxes, `GpuMesh` buffers shared via `Arc`, one dynamic-offset uniform buffer for all shadow draws). Never `create_buffer`/`create_bind_group` inside a per-entity loop; a 270-entity scene went from ~17 ms to ~4 ms when those were removed. Skinned draws use the same cache (`SKINNED_DRAW_KEY`, bone binds in `skinned_bone_binds`, #121). On the web every `create_*` is also a JS object: Float's skinned path made 386 buffers and 327 bind groups a frame, and Firefox stalled ~250 ms at each GC. Count WebGPU calls per frame in the browser before trusting a desktop timing
- Offline capture: `flint-player --frame-step S` advances the clock exactly S seconds per frame (wall time ignored), and with `--screenshot-every S` every frame is written; `--input-script FILE` plays timed `down`/`up`/`tap` keys, `axis` values and gamepad `press`/`release` (on pad slot 9) at game times (format in `player_app/input_script.rs`); a scripted key also goes through the debug keys (`handle_debug_key`), so `tap F2` opens the stats overlay, and `--stats` opens it from the first frame. Together they make deterministic, smooth gameplay footage and scripted regression runs
- Perf switches: `flint-player --exit-after N` prints `[perf] avg ms/frame`, the spread of dt and of present-to-present times (p50/p90/p99/max and frames over 1.5x p50), and the average draw calls, entity draws, culled draws and triangles on exit; `FLINT_VSYNC=0|mailbox` picks the present mode (Fifo can pace at a virtual display's rate, e.g. 7 Hz with Virtual Desktop attached); `FLINT_FRAME_LATENCY=1..3` sets the swapchain's frame latency (default 2); `FLINT_PACING=0` turns off vsync pacing; `FLINT_SHADOWS=0` disables the shadow pass at start; `--render-scale 0.25..1|auto` draws the 3D scene at that fraction of the window on each side (HDR chain + its own depth, upscaled by the composite, depth copied back so world text/lines still test against it) under a full-resolution HUD, `auto` stepping down to hold 60 fps; the web caps the scene at DPR 1.5 by default and takes `?scale=` (#122, `SceneRenderer::set_render_scale`); `--perf` prints a line every 10 s with each frame phase's CPU time, the worst scripts and each pass's GPU time, and the web page's `?perf=1` (`?perf=cpu` without GPU timing) shows the same in a corner with a Copy line button (#120, docs/design/web.md)
- Vsync pacing: under Fifo the player gives `GameClock` the monitor's refresh period (`set_vsync_period`, re-read when the window moves), and `delta_time` comes in whole refresh periods that keep game time on the wall clock. On a hybrid-GPU laptop Fifo lets frames through in bursts (6, 6, 6, then 45 ms at a 60 Hz mean); paced, every frame steps 16.7 ms. Stalls over 250 ms pass through, and frames faster than the refresh on average fall back to raw time. `raw_delta_time` is the unpaced wall time. See the book's Profiling guide
- CI's `wasm32 check` (`.github/workflows/ci.yml`) builds `flint-player` and every crate it links for `wasm32-unknown-unknown` on each PR. Run it locally with `bash scripts/wasm-check.sh` before pushing runtime-crate changes. It also denies `Instant::now`/`SystemTime::now` and their `elapsed()`, which compile on wasm but panic there: runtime code uses the `web-time` crate (a workspace dep: `web-time = { workspace = true }`), which #100 moved every older site to. Desktop-only deps in a runtime crate go under `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]`, as in flint-player's Cargo.toml (docs/design/web.md)
- Web frame loop (#121): `flint_web.rs` runs winit with `ControlFlow::Wait` and `about_to_wait` requests a redraw, so the player draws once per `requestAnimationFrame` and the main thread sleeps between frames. Never `ControlFlow::Poll` on the web: it chains `scheduler.postTask` tasks back to back, Chrome shrugs but Firefox starved its refresh driver (83 ms frames, multi-second stalls, half the audio buffers late). `?loop=poll` brings the old loop back for comparisons
- Web player (#102): draw the swapchain through `RenderContext::frame_view` (sRGB view of a plain WebGPU canvas), never a default view. Sound and pointer lock need a gesture: the page's Begin calls `flint_player::web_begin()`, and on wasm `set_cursor_captured(true)` is a wish that `sync_pointer_lock` turns into a lock while the page has activation; mouse look flows only while the lock is held. Headless Chrome never grants pointer lock, and headed Chrome only with the window focused (`Page.bringToFront`)
- play.chaps.dev (#103): `.github/workflows/play.yml` builds the player with trunk, packs every game in `tools/play-site/games.toml` (each pinned to a commit) with `flint bundle`, and deploys this repo's Pages from Actions. `?game=<id>` plays `games/<id>/<id>.flintpack`; `?embed=1` is the iframe view. `index.html`'s doorway shows the chooser, or the game's film when WebGPU is missing, before any wasm loads: `Trunk.toml`'s loader awaits `window.flintDoorway` and preloads only the JS glue. Keep the two together, or a browser that can't play downloads 7 MB of wasm (docs/design/web.md)
- Web size (#123): the site builds with `trunk build --release --cargo-profile web-release` (workspace `[profile.web-release]`: opt-level z, fat LTO, 1 codegen unit, panic = abort; 3.1 MB gzipped, down from 5.1). The workspace `image` dep is PNG/JPEG/HDR only and `flint-render` restores image's defaults natively, so add a format for desktop there, not in the workspace. rhai's `no_optimize`/`no_custom_syntax`/`only_i64` are on for wasm only (flint-script's Cargo.toml): web scripts see `i64` integers and nothing else. Measure a size change with trunk and record it in web.md's Size table
- `FlintWorld::spawn` ids come from a process-wide counter and scenes load from a `HashMap`, so entity ids differ run to run -- derive anything reproducible (seeds) from entity **names**, never ids

## Documentation Pipeline

The book is `docs/book` (mdBook), published to docs.chaps.dev. It is kept in sync by a deterministic checker plus an agent playbook; do not sweep it by hand from memory.

```bash
python tools/docs-check/check.py              # drift report; exit 0 clean, 1 drift
python tools/docs-check/check.py --report md  # same, as a PR-body block
mdbook build docs/book                        # must be clean before any docs commit
```

- `docs/book/.synced-to` holds the engine SHA the book was last verified against. Update it in every `docs:` commit.
- `docs/DOCS_SYNC.md` is the playbook (ownership map, rules against inventing keys, commit style). The `docs-sync` skill and `.github/workflows/docs-sync.yml` both follow it: a push to `main` touching `crates/`, `schemas/`, `CLAUDE.md` or `Cargo.toml` runs the checker, and on drift Claude Code opens or refreshes a `docs-sync` PR.
- `.github/workflows/docs-publish.yml` rebuilds mdBook + rustdoc and pushes docs.chaps.dev when `docs/book/**` lands on `main`. `docs/build.ps1` remains for local `-Serve` previews.
- Renders for the book are produced locally (no GPU in CI) via the `flint-render` skill and land in `docs/book/src/images/` in kebab-case; `renders/` is scratch and gitignored.
- When a keybinding is retired, add it to `DEAD_PATTERNS` in the checker so it cannot creep back.

## Project Structure

- `crates/` -- All workspace crates
- `schemas/` -- Engine component and archetype TOML definitions
- `styles/` -- Style guide TOML definitions for AI generation
- `demo/` -- Showcase scenes, scripts, audio, animations, `particles/` effect assets
- `testGame/` -- Test project (levels/, schemas/)
- `docs/book/` -- mdBook source; `docs/DOCS_SYNC.md` -- docs playbook; `tools/docs-check/` -- drift checker
- `docs/design/flint-design-doc.md` -- Full design document for remaining phases

<!-- claims:begin (from chrischaps/Dashboard skills/setup-claims; re-run its install.sh to update) -->
## Claiming issues

Agents on several machines work this repo, all as the same GitHub user, so an issue is claimed before anyone works on it. A claim is the branch `claim/issue-<n>` on origin, holding one empty commit that names the agent and host. Git accepts only the first push of a new branch, so exactly one claimant wins. `scripts/issue.sh` does the whole cycle (bash; on Windows it runs under Git Bash):

```bash
scripts/issue.sh list                  # who holds what, on every machine, and for how long
scripts/issue.sh claim <n>             # claim and make the worktree ../Flint-<n>; exit 2: taken, pick another
scripts/issue.sh claim <n> --here      # claim for the worktree you're already in (a desktop-app worktree)
scripts/issue.sh renew <n>             # at the start of each session on long work
scripts/issue.sh land <n>              # verify, push the branch and open a PR to main; release <n> once it merges
scripts/issue.sh release <n>           # giving up: push your branch and comment where it stands first
```

- **A hook enforces it.** `.claude/settings.json` runs `scripts/issue-guard.sh` before every `git commit` and `git push`. It refuses issue work (a branch named `issue-<n>`, or a commit saying `Closes #<n>`) unless this session holds the claim, from the worktree it was claimed for. A chore with no issue (a typo, a dependency bump) needs no claim: keep it off `issue-<n>` branches and out of `Closes` lines. A SessionStart hook lists the current claims.
- **Only the session that claimed an issue renews, releases or lands it,** and only from its worktree. An existing worktree for an issue you didn't claim is another session's: leave it alone. Never delete another agent's `claim/*` branch.
- **Claims go stale after 24 hours.** `claim <n> --steal` takes over a stale one; check the issue's comments and `origin/issue-<n>` for work in progress first, and continue from it. Taking a younger claim is Chris's call only: `claim <n> --steal --override "<his reason>"`.
- Settings for this repo are in `scripts/issue.conf`. The tool is vendored from `skills/setup-claims` in chrischaps/Dashboard: change it there, not here. `bash scripts/issue-test.sh` tests it against a throwaway remote.
<!-- claims:end -->
