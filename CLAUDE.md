# CLAUDE.md

This file provides guidance to Claude Code when working with this repository.

## Project Overview

Flint is a CLI-first, AI-agent-optimized 3D game engine written in Rust. The primary interface is CLI and code; visual tools validate results rather than create them. Phases 1-5 complete (ECS, constraints, PBR rendering, scripting/physics/audio/animation, AI asset generation). Game projects live in their own repositories with the engine as a git subtree.

## Build & Development Commands

```bash
cargo build                    # Debug build
cargo build --release          # Release build
cargo build -p flint-cli --no-default-features  # Headless CLI: no sound card, gamepads or file dialogs,
                               #   so no ALSA/libudev headers on Linux (#221; features audio/gamepad/dialogs;
                               #   `bash scripts/headless-check.sh [--build]` keeps device crates out of its tree)
cargo run --bin flint -- <cmd> # Run CLI
cargo run --bin flint -- play flint-examples/phase4_runtime.scene.toml  # Play a scene
cargo run --bin flint-player -- flint-examples/phase4_runtime.scene.toml  # Standalone player (schemas: the root flint.toml)
cargo run --bin flint -- export web flint-examples/  # any game as a folder a static server plays (#106):
                               #   flint-examples/dist/web = player + games/<id>/ bundles + games.json;
                               #   --out, --player <trunk dist> (else trunk builds it), --meta k=v, --clean
cargo test                     # Run all tests
cargo test -p flint-core       # Test a single crate
cargo clippy                   # Lint
cargo fmt --all                # Format (rustfmt.toml pins the edition; `rustfmt <file>` for one file)
bash scripts/ci.sh             # What a PR must pass: formatting, a record for every cited ADR
                               #   (scripts/adr-check.sh), every clippy.toml made from the root's
                               #   (scripts/clippy-conf.sh --check) and the tree's size budget
                               #   (scripts/size-check.py, ADR 0122), in seconds. `issue.sh land` runs it.
bash scripts/ci.sh --full      #   ...plus what CI's clippy and wasm jobs run (two builds)
```

## Build output across worktrees

Every agent worktree builds its own `target/`, and parallel worktrees are how this repo is worked, so disk is what runs out (#762):

- **Keep `target/` per worktree; never point several at one `CARGO_TARGET_DIR`.** Cargo names a workspace crate's output by its path inside the workspace, so two worktrees' `flint-render` share one output and a branch can build against the other's (2026-10-30: a branch failed on a method its own source defines). One target dir also has one build lock, so parallel agents queue, and tests that write into it collide (#421). An A/B against another revision gets its own dir.
- **The dev profile is lean** (`Cargo.toml`): line tables for workspace crates, no debug info for dependencies. A cold `cargo build` plus `cargo test --no-run` measures 6.6 GB and 150 s. Locals in a debugger: `CARGO_PROFILE_DEV_DEBUG=full`. What loads a texture is built at `opt-level = 3` even in debug (#326): the image decoders, glTF, SHA-256 and `flint-pixels`, where the texel loops live so `flint-render` stays unoptimized. A debug render of a scene with a 32 MB model takes about 2.6 s; a new per-texel loop on load goes in `flint-pixels`. So is egui's text stack (epaint, harfrust, skrifa, vello_cpu, #1129): unoptimized, the first F2 press cost 110 ms.
- **sccache is optional, per machine, not in `.cargo/config.toml`** (a wrapper that isn't installed fails every build). With `RUSTC_WRAPPER=sccache` a cold-cache build measured no slower (96 s); what a warm cache saves the next worktree is unmeasured (the run was stopped for low memory). It can cache only dependencies, since the workspace crates build incrementally.
- **Disk is guarded** by `scripts/build-space.sh`. A hook refuses `issue.sh claim` under 30 GB free (`FLINT_DISK_FLOOR_GB`), and the SessionStart hook deletes the `target/` of worktrees whose issue is closed (not the worktree, not one built in the last hour). `report` lists every worktree's `target/` by size. A release build adds several GB: before one, check `df -h .`, and delete your own worktree's `target/release` when you're done with it. So does `--profile profiling` (#1101, release plus line tables for a sampling profiler): `-p flint-player` cold measured 2.7 GB and 3 min 46 s in its own `target/profiling` (a 187 MB PDB), shared with nothing; delete it when done (book: guides/profiling.md, "When scopes aren't enough").

## Development Workflow

Flint uses an **edit -> validate -> play** loop:

1. **`flint render`** -- Headless scene-to-PNG snapshot. Primary validation tool for AI agents.
   ```bash
   flint render levels/demo.scene.toml --output test.png --width 1280 --height 720
   flint render levels/demo.scene.toml --output test.png \
     --distance 20 --pitch 30 --yaw 45 --target 0,1,0 --no-grid
   ```
   Every flag: `flint render --help`, `--list-styles --json`, `--look help`, and `crates/flint-cli/CLAUDE.md`.

2. **`flint edit <file>`** -- Unified interactive editor. Auto-detects file type and opens the right tool:
   ```bash
   flint edit levels/demo.scene.toml              # Scene viewer (hot-reload)
   flint edit models/character.glb                 # Model previewer (orbit camera)
   flint edit specs/oak_tree.procgen.toml          # Procgen previewer (mesh/texture)
   ```
   More: `crates/flint-cli/CLAUDE.md`.

3. **`flint gen`** -- Run a procedural generation spec to produce meshes (GLB) or textures (PNG).
   ```bash
   flint gen specs/oak_tree.procgen.toml -o tree.glb
   flint gen specs/stone_wall.procgen.toml -o wall.png
   flint gen specs/oak_tree.procgen.toml --dry-run
   flint gen flint-examples/rolling_meadow.terrain.toml -o terrain/   # heightmap + splat PNGs, no window
   ```
   More, and the goldens: `crates/flint-cli/CLAUDE.md`.

4. **`flint play`** -- Full game runtime with physics, scripting, audio, and input.
   ```bash
   flint play flint-examples/phase4_runtime.scene.toml
   flint play flint-examples/phase4_runtime.scene.toml --width 1920 --height 1080
   ```
   More: `crates/flint-player/CLAUDE.md`.

5. **`flint capture`** -- A shot list (`[[shot]]` scene, seconds, look, input script) captured as
   deterministic frame sequences, each take a stepped `flint play`; `--verify` runs each twice and
   names the frames that differ, `--encode mp4` with ffmpeg. More: `crates/flint-cli/CLAUDE.md`.
   ```bash
   flint capture showcase_game/trailer.shots.toml --verify --encode mp4
   ```

**For AI agents:** Before playing, rendering or bundling, `flint check <dir|scene> [--json]` is the pre-flight (#687): every problem of a project in one run, each with its file, entity, field, a hint and the paths tried, exit 1 on an error, scripts included (`flint script check` alone gives each script's mistakes as `file:line:col`, #694; `crates/flint-cli/CLAUDE.md`). Every command that opens no window takes the global `--json` (#710): exactly one JSON document on stdout, everything else on stderr, exit 1 on a failure with the report or an `{"error": ...}` document still on stdout; read answers from it, not from the text (`crates/flint-cli/CLAUDE.md`, "Output"). After a gameplay change, `flint playtest <scene> --input-script <file>` (#714) plays it headless on a fixed clock and fails on what didn't happen: an input script's `expect` / `wait` lines, the scripts' `test_assert` / `test_fail`, any script error; one JSON report, byte-identical run to run (`crates/flint-player/CLAUDE.md`, book: guides/playtesting.md). After visual changes, always run `flint render` to verify before moving on. The render command loads models (found by the project's one asset search: `models/` beside the scene, beside each directory up to the project root, then `engine/`; `flint asset where models <name> --scene <scene>` shows it), generates geometry, and computes the full transform hierarchy.

## Architecture

32-crate Cargo workspace (plus `tools/arch-analyzer`, a syn-based dependency/metrics extractor feeding the `tools/arch-viewer` web graph and the book's crate table, checked in CI). Every member but `flint-android` is a default member, so plain `cargo test` runs every desktop crate's tests (#758):

```
flint-cli           CLI binary (clap). Commands: init, entity, scene, query, schema, script, edit, play, playtest, capture, check, frame, profile, env, validate,
  │                 examples, asset, render, style, gen, prefab, bundle, export, music, goldens, dxc
  │                 (`music`: validate, play, render, calibrate, chart play|replay, #709; hidden aliases: serve, preview,
  │                 gen-preview, spline-edit, terrain-edit, tex-edit, and for one release the old music names, spike-rumble)
  ├── flint-asset-gen AI asset generation: pluggable providers (Flux, Meshy, ElevenLabs, Mock)
  ├── flint-procgen   Procedural generation: Generator trait, registry, specs, GLB export
  ├── flint-procgen-ai AI-assisted procgen: ProcGenAgent trait, spec creation/refinement
  └── flint-ai-core   What every paid AI call goes through: transport + one retry policy, prices.toml (+ .flint/prices.toml), the .flint/ai_costs.jsonl ledger, CostMeter (--max-cost)
flint-android       Android entry point (NativeActivity, APK asset extraction)
flint-player        Standalone player (game loop, physics, audio, animation, scripting, egui HUD)
  ├── flint-rhythm   The rhythm layer: charts, judgment, coherence, ladder/seam/reintegration, replay, haptics
  ├── flint-music    Music: suite manifests with manifest-named buses, tempo maps, conductor, scheduler, offline render
  ├── flint-input-capture 1 kHz gamepad capture thread (gilrs XInput) stamped against the audio clock; verb maps
  ├── flint-debug-ui DebugPanel trait + F3/F4 panel roster (Rendering & Effects, ocean, sky, camera, grass, particles, Living Props...); shared `widgets` (drag helpers, CurveEditor, GradientEditor); optional via debug-hud
  ├── flint-script   Rhai scripting with hot-reload
  ├── flint-particles Particle sim: effect assets, curves, forces, bursts, sub-emitters, deterministic stepping
  ├── flint-animation Property tween + skeletal animation
  ├── flint-audio    Kira spatial audio
  ├── flint-audio-worklet Kira mixer for the browser's AudioWorklet (its own wasm, `--profile worklet`)
  ├── flint-terrain  Heightmap terrain with splat-map blending
  ├── flint-physics  Rapier 3D (character controller, static/kinematic/dynamic bodies, impulse joints)
  ├── flint-runtime  GameClock, InputState, InputConfig, EventBus, GameStateMachine, PersistentStore, pad (gilrs), PadRumble
  ├── flint-viewer   egui inspector with transform gizmos, undo/redo, TOML write-back
  ├── flint-import   glTF model importer
  ├── flint-asset    Content-addressed asset catalog
  ├── flint-constraint Constraint evaluation + auto-fix
  ├── flint-query    PEG query language (pest parser)
  ├── flint-scene    TOML scene serialization/deserialization
  ├── flint-render   wgpu PBR renderer (winit 0.30, wgpu 30, HDR post-processing, bloom, SSAO, fog)
  ├── flint-pixels   Texel loops on load (RGBA widening, mip chains); no deps, optimized in debug builds
  ├── flint-ecs      The world: stable EntityIds, dynamic components, names, hierarchy, component index
  ├── flint-schema   Component/archetype schema system
  ├── flint-wooze    The wooze field (how woozy a place is, as a function of position and time) and the boil clock
  ├── flint-geometry Gerstner ocean spectrum, spline sampling and `.spline.toml` files, profiles swept along them, rays against drawn meshes; their component names
  └── flint-core     EntityId, ContentHash, Transform, Vec3, Color, FlintError, vfs (read-only file layer)
```

## Where the detail lives

Feature notes go in the nearest scoped CLAUDE.md, not in this file. When a ticket says 'update CLAUDE.md', it means the scoped file for the code you changed; add to this root file only what every session needs.

| Scoped file | Covers |
|---|---|
| `crates/flint-cli/CLAUDE.md` | `flint render` flags, `edit`, `gen`, goldens, export, Linux |
| `crates/flint-render/CLAUDE.md` | post-processing, quality, boil, shallow wooze, glyph atlas, caches, lights |
| `crates/flint-render/src/stylize/CLAUDE.md` | styles, style mask, per-entity and user styles |
| `crates/flint-procgen/CLAUDE.md` | living props, wooze field, Woozy Grove |
| `crates/flint-player/CLAUDE.md` | extensions, UI, input, touch, capture, perf, web |
| `crates/flint-script/CLAUDE.md` | Rhai detail |
| `crates/flint-core/CLAUDE.md` | vfs, hot reload, shear |
| `crates/flint-audio/CLAUDE.md` | audio clock, decoding, device features |
| `crates/flint-{physics,animation,particles,terrain}/CLAUDE.md` | joints, IK, particles, terrain |
| `flint-examples/tools/showcase/CLAUDE.md` | film rig (in the examples' repository, #984) |
| `docs/CLAUDE.md` | book build and publish |

A bullet below that names a feature in a line or two has its full text in one of these.

## Key Design Principles

- **Dynamic components** stored as `toml::Value` -- archetypes defined at runtime in schema TOML files, not Rust types
- **Stable EntityIds** via atomic counter (never recycled), persisted across save/load
- **Scene format is TOML** -- `[scene]` metadata + `[entities.<name>]` sections; reload is full re-parse
- **Schemas in `schemas/`** -- `components/*.toml` for field types, `archetypes/*.toml` for bundles, `scene/*.toml` for scene-level tables (`[table.<name>]`: `[post_process]`, a game's own `[weather]`; #686, `crates/flint-core/CLAUDE.md`). Several directories load in order, later overriding earlier (game overrides engine): a later `[component.x]` replaces the earlier whole, unless it says `extend = true`, which adds its fields to it (#794; `SchemaRegistry::component_origin` says which file each field came from). One resolver names them for every command, the viewer and the player (ADR 0114; `crates/flint-core/CLAUDE.md`): repeatable `--schemas` flags if given (the whole list), else `[project] schemas` in the project's `flint.toml`, else `schemas/` in the working directory, beside the scene and one level up
- **Scripting via Rhai** -- `script` component with `.rhai` files in `scripts/`; callbacks: `on_init`, `on_update`, `on_collision`, `on_trigger_enter/exit`, `on_action`, `on_interact`, `on_draw_ui`, `on_scene_exit/enter`, `on_animation_end`, `on_sequence_cue`, `on_event`, `on_expression(state)`, `on_persist_migrate(from)`; `ScriptCommand` for deferred effects; `DrawCommand` for immediate-mode 2D UI
- **All game UI is script-driven** -- no hardcoded HUD; `hud_controller` entity + `hud.rhai` pattern; engine provides draw primitives (`draw_text/rect/circle/line/sprite`), scripts compose UI. A menu is a UI document (`.ui.toml` + `.style.toml`, `load_ui`): `button`/`list`/`slider`, focus from keys, pads, mouse and touch, `on_ui(element_id, event)`, `scripts/lib/menu.rhai`; a mistake in one is a script error and `flint check`'s `bad-ui`; `flint render --scripts --ui-show <id>` draws it headless (#731; `crates/flint-script/CLAUDE.md`, the book's "When to use each UI system")
- **Game project pattern** -- games in own repos with engine as git subtree at `engine/`; `schemas = ["engine/schemas", "schemas"]` under `[project]` in the game's `flint.toml` layers the game's schemas over the engine's with no flags, and `--schemas engine/schemas --schemas schemas` overrides it for a run
- **Player extensions** (ADRs 0071-0075) -- a game binary embeds `PlayerApp` and installs `PlayerExtension`s (`add_extension`): native Rhai fns, `pre_update` before scripts' `on_update`, window events, egui UI. Book: guides/player-extensions.md
- **Every runtime read goes through `flint_core::vfs`** (#97) -- `vfs::read` / `read_to_string` / `read_dir` / `exists` / `is_file` / `is_dir`, never `std::fs` or `Path::exists` in a runtime crate. Decode images with `flint_render::open_image` and glTF through `flint_import::import_gltf`. Tests and the CLI keep `std::fs`. Plain `cargo clippy` warns on a read past the vfs and on `as_float` in every crate the player links (the root `clippy.toml`, #761; denied in the five that draw): a write path or test that means it says why, `#[allow(clippy::disallowed_methods)] // <reason>`, a test file once at its top (`#![allow(..)]`), a crate's unit tests in one `#![cfg_attr(test, allow(..))]` in its lib.rs. Change a rule in the root `clippy.toml`, then run `bash scripts/clippy-conf.sh`: it writes the tool crates' own copies (`TOOL_CRATES`: the CLI, viewer, asset-gen, procgen-ai, ai-core and arch-analyzer hold only the wgpu seams) and `scripts/ci.sh` fails a stale one. The clock rules can't be native (there `web_time::Instant` is `std::time::Instant`): `scripts/wasm-clippy/wasm-only.toml`, added for the wasm check alone
- **`FLINT_*` environment switches are one table** (#715, `flint_core::env`): each is a `Var` (name, kind, default, help, group) read only through its getter (`env::VSYNC.choice()`, `env::LOD_SCALE.f32()`, `.flag()`, `.path()`), which warns once on a value it doesn't take; `flint env [--json]` lists them with their values, and the `--help` of `render`, `play` and `flint-player` ends with theirs. `scripts/ci.sh` refuses a `var("FLINT_` elsewhere, and `cargo test -p flint-core --test env_table` a `"FLINT_..."` name the table lacks: a new switch is a row there, then `flint env --book`. `crates/flint-core/CLAUDE.md`
- **Fixed-timestep physics** (1/60s) via accumulator; animation/particles run at variable rate

## Critical Gotchas

- `toml::toml!` macro produces `Map<String, Value>` not `Value` -- needs `.into()` in tests
- wgpu v30 (#945; egui 0.36): `Instance::new()` takes owned `InstanceDescriptor` (it has no `Default`: start from `InstanceDescriptor::new_without_display_handle()`); max 4 bind groups (0-3); `push_error_scope` returns a guard that pops the scope when it drops, so bind it and `.pop()` it; a frame is shown with `queue.present(frame)`; block on the GPU with `flint_render::wait_for_gpu(&device)`, never `PollType::wait_indefinitely()` (clippy holds it; `FLINT_GPU_WAIT_TIMEOUT` caps every wait, #948)
- winit v0.30: `ApplicationHandler` trait with `run_app()`; `DeviceEvent::MouseMotion` for raw mouse delta
- Rapier v0.22: character controller types in `rapier3d::control`, NOT `rapier3d::prelude`
- Kira v0.11: `Decibels(f32)` and `PlaybackRate(f64)` are tuple structs; uses `glam` via `mint`
- `AudioManager::new()` / `Gilrs::new()` may fail in headless/CI -- wrap in `Option`
- Rhai v1.24 `sync`: `call_fn()` Scope visible only to direct callee, NOT sub-functions; entity IDs as `i64`
- `flint-android` is the one crate excluded from `default-members` (a new crate goes in both lists, #758) -- use `cargo ndk -p flint-android`; min API 26. `cargo test --workspace` fails on `ndk-sys` on Windows; use plain `cargo test`
- Windows links a 1 MB main-thread stack; `.cargo/config.toml` links debug builds with `/STACK:8388608`. Don't remove it: the commands' own stack use is unmeasured. The clap parser was what overflowed (a derived `augment_args` is one frame of about 8 KB a flag in debug: 1346 KB while `flint render` was one 130-flag struct, about 600 KB (579 to 643 as commands come and go) since #708 made a struct a help heading and boxed every `Commands` variant). Keep an `Args` struct to a dozen or so flags; `stack_tests::the_parser_fits_in_windows_default_stack` in `main.rs` fails past 1 MB.
- Scene entity `merge_component()` does field-level merge INTO archetype defaults, not full replacement
- Read a component through `world.view(id, comp::COLLIDER)` (`.f32("friction", 0.5)`, `.choice("shape", &[..], "box")`, `.opt_f32(..)`; ADR 0115, `crates/flint-core/CLAUDE.md`): it takes an integer as a number and reports a wrong type or unknown choice once per entity and field. Any other TOML goes through `flint_core::toml_util` (`toml_f64`/`toml_f32`/`toml_vec3`/...). Never `toml::Value::as_float` in a runtime crate: it reads `8` as nothing, and `cargo clippy` warns on it (the root `clippy.toml`, #761) and `scripts/wasm-check.sh` denies it
- `furniture`, `wall`, `floor`, `ceiling`, `pillar`, `room`, `door` (and the `door` component) are not the engine's: they are the examples' layer, `schemas/examples/` (#681). The root `flint.toml` lists it after `schemas/`, so every scene in this checkout resolves them with no flag; a `--schemas` flag is the whole list, so `--schemas schemas` alone leaves them out (add `--schemas schemas/examples`), and a game has none of them unless its own `flint.toml` lists `engine/schemas/examples`. In an example, never put `furniture` on a non-visual entity: it includes `bounds` and draws a green box. A `bounds` box's colour is its archetype's `[archetype.<name>.visual]` (`color`, `wireframe`; #751), which each host hands the renderer with `SceneBinding::with_archetypes(&registry)`; the renderer names no archetype, and one with no `visual` draws grey
- Every `flint_core::components` constant has a schema in `schemas/components/`, and every schema a reader: `cargo test -p flint-schema --test engine_schemas` holds it (ADR 0115; what it checks: `crates/flint-core/CLAUDE.md`). A schema field's `default` is written into the component at load, so give one only where the reader falls back to that same value
- Per-frame GPU allocations are the renderer's CPU cliff: never `create_buffer`/`create_bind_group` inside a per-entity loop; what a draw needs per entity comes from a cache (`crates/flint-render/CLAUDE.md`)
- One submission a frame (#139): `SceneRenderer::encode_to` records the whole frame (grass, shadows, scene, post-process, depth upscale, world lines) into the caller's encoder, the player adds its HUD and the GPU timer's resolve (`encode_gpu_timings`), submits once, then calls `frame_submitted` (grass and timer read-backs). `render_to` is that with its own submit.
- CI (`.github/workflows/ci.yml`, #757) runs `rustfmt` (the tree is formatted; an unformatted file fails the PR and `issue.sh land`), `clippy (native)` (`cargo clippy --all-targets`: warnings pass, a deny-level lint or code that doesn't compile fails), the wasm and headless checks, and the GPU tests on WARP, which don't block while they fail on main (`continue-on-error`). No unit tests run in CI yet
- CI's `wasm32 check` (`.github/workflows/ci.yml`) builds `flint-player` and every crate it links for `wasm32-unknown-unknown` on each PR. Run it locally with `bash scripts/wasm-check.sh` before pushing runtime-crate changes. It also denies `Instant::now`/`SystemTime::now` and their `elapsed()`, which compile on wasm but panic there: runtime code uses the `web-time` crate (a workspace dep: `web-time = { workspace = true }`), which #100 moved every older site to. Desktop-only deps in a runtime crate go under `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]`, as in flint-player's Cargo.toml (docs/design/web.md)
- A scene's entity ids are the same every load (#688): each `FlintWorld` counts its own from 1 and a scene spawns in name order, so ids are the names' ranks in any process; every walk of the world is ordered (`entities_with_component` by id, `all_entities` by name). A host that replaces its world (a transition, an editor reload) starts past the old one's ids with `FlintWorld::after` / `flint_scene::load_scene_after`, so a kept id never names a new entity; there is no `EntityId::new()`. Ids still move when an entity is added or renamed, so a seed meant to survive edits comes from the entity's **name**
- `world.get_components_mut(id)` returns a `ComponentsMut` guard, not `&mut DynamicComponents` (#691): bind it `mut`; its `set`/`set_field`/`merge_component`/`remove`/`set_archetype` keep the component index, and there is no way to the map underneath, so there is no index rebuild. One component's value to change in place: `world.get_component_mut(id, name)`. `despawn` takes the entity's whole subtree and returns the ids removed; reparent a child first to keep it
- Lights are `[entities.x.light]` with a `type`, and a directional `direction` points *toward* the light; with no `direction` a light shines down its entity's -Y and turns with it, and spot cones are `inner_angle_deg`/`outer_angle_deg` (#682; `crates/flint-render/CLAUDE.md`). `flint schema light` lists the fields. A `directional_light` table is not a light: no system reads it, `flint validate` and `flint check` warn about it as an unknown component (with a hint naming `[light] type`), and a scene with only that gets `default_scene_lights`. - Lights are `[entities.x.light]` with a `type`, and a directional `direction` points *toward* the light; with no `direction` a light shines down its entity's -Y and turns with it, and spot cones are `inner_angle_deg`/`outer_angle_deg` (#682; `crates/flint-render/CLAUDE.md`). `flint schema light` lists the fields. A `directional_light` table is not a light: no system reads it, `flint validate` and `flint check` warn about it as an unknown component (with a hint naming `[light] type`), and a scene with only that gets `default_scene_lights`. No scene in the repo carries one, the frozen fixtures included (#796: `engine_schemas` and `crates/flint-cli/tests/fixture_scenes_cli.rs` hold it).

## Documentation Pipeline

The changelog is for games that carry the engine as a subtree (#765): a change a game can notice (a scene or schema key, a script function, a CLI flag, a format, a Cargo feature, a change in look or behaviour) gets an entry saying what changed and what to do. Each PR adds its own file, `bash scripts/changelog.sh new <issue> <slug>` (`changelog.d/<issue>-<slug>.md`, about 150 words), never an edit to `CHANGELOG.md` or `changelog/` (#788): after the merge the `Changelog` workflow moves it into `changelog/YYYY-MM.md` and `CHANGELOG.md` stays a short page linking the months. `changelog.sh since <date>` prints what is new. CI's `changelog` job (`scripts/changelog-check.sh`) fails a PR that touches `schemas/`, the script API or the scene format with no fragment; a commit-message line `Changelog: none (reason)` waives it for a change no game can notice.

The book is `docs/book` (mdBook), published to docs.chaps.dev by hand from this machine. CI checks its generated parts, the script API (`flint script api --check-book`) and the crate table (`flint-arch-analyzer --check-book`), and `python scripts/docs-check.py` (#375) checks what the prose names against a built `flint`: every `--flag` on a `flint` command line (book and CLAUDE.md files), every `[post_process.styles.<name>]`, every image a page links, and the maps (#756): a crate or member count (in digits), every crate in this file's Architecture tree and the book's overview, and a `Commands:` list against `flint --help`. Nothing checks what the prose says a flag or key does. When a change alters a command, flag, key or component the book describes, update the book in the same PR, checking against the code rather than memory. There is no `tools/docs-check`, `DOCS_SYNC.md` or `.synced-to` (#175, #375): a ticket that names them is wrong.

Build, preview and publish commands: `docs/CLAUDE.md`.

## Project Structure

- `crates/` -- All workspace crates
- `schemas/` -- Engine component, archetype and scene-table TOML definitions; `schemas/examples/` -- the room-builder layer the examples are written in, not part of the engine's set
- `flint.toml` -- the checkout as a project: `schemas = ["schemas", "schemas/examples"]`, read by everything run from this tree (a game with the engine as a subtree never reads it)
- `styles/` -- AI style guides (`*.styleguide.toml`, #420; `*.style.toml` is a UI class file) and looks (`*.look.toml`)
- **The examples are another repository** (#984, ADR 0131): [flint-examples](https://github.com/chrischaps/flint-examples), the curated scenes at its root with the engine as a squashed subtree at `engine/`, like a game, and its own CI (`flint examples --check`). Clone it into this checkout, where `.gitignore` keeps it out (`git clone https://github.com/chrischaps/flint-examples`), and every `flint-examples/<path>` in this tree is a path: `flint play flint-examples/phase4_runtime.scene.toml` from the root. Nothing here may name the old in-tree folder (`scripts/examples-check.sh`, in `scripts/ci.sh`), and docs-check holds every `flint-examples/<path>` to a file there. Its schemas are its subtree's, so to run one against this tree's: `--schemas schemas --schemas schemas/examples`. An engine feature with an example bench is two PRs: the engine's (its test in `tests/fixtures/scenes/`), then flint-examples' (a `git subtree pull` and the bench)
- `tests/fixtures/scenes/` -- Frozen copies of the scenes the goldens and CLI tests load, with every file they read (#984 took back the 26 binaries they had borrowed from the examples). No golden may load an example (`no_golden_case_loads_an_example`); a change here is a change to a test
- **Every byte here is in every game** (ADR 0122, #759): a subtree is the whole tree. `scripts/size-check.py` (in `scripts/ci.sh`) fails a PR on a binary tracked twice, a texture over 2048 texels a side or 3 MB (`tools/assets/cap_textures.py` shrinks one with the renderer's own mip filter; colour maps as JPEG), a file over 8 MB, a book PNG with an alpha channel (`tools/assets/pack_book_images.py` drops it, same pixels; #1276), or the tree or one of its parts over budget; `--report` prints the sizes
- `showcase_game/` -- The Overgrown Ruins, the example game: a project with its own `flint.toml`, assets and sounds (its bartender's model is its own copy, #984), the engine beside it (`../schemas`) rather than under it. A top-level directory with a `flint.toml` is a game kept in the checkout; tests find one by its manifest, so engine code never names it (#760), and `crates/flint-cli/tests/checkout_games_cli.rs` fails when one names a file it doesn't have
- The root holds only manifests and docs, and nothing is a symlink (a Windows checkout without the privilege gets a text file): `scripts/ci.sh` refuses either. A default `flint render` writes `render.png` to the working directory, which `.gitignore` covers, as it does coverage runs' `*.profraw`
- `crates/flint-cli/templates/` -- What `flint init` writes (#707): the game-project scaffold, compiled into the binary; `tests/init_cli.rs` builds and runs each
- `docs/book/` -- mdBook source; `docs/build.ps1` / `docs/publish.sh` -- build, preview and publish it
- `docs/design/` -- Living design notes (the web port, the showcase film, Living Props targets, spike and cost measurements); `docs/archive/` -- superseded plans and the March 2026 design doc, each with a banner naming the book page that is current (#756). A plan whose work has shipped moves to the archive
- `docs/adr/` -- Architecture decision records, one file per number cited anywhere in the tree (0111 on written here; earlier ones rebuilt from commits and the Starchild repo, #374); `README.md` says how to take a number

<!-- claims:begin (from chrischaps/Dashboard skills/setup-claims; re-run its install.sh to update) -->

## Claiming issues

Agents on several machines work this repo, all as the same GitHub user, so an issue is claimed before anyone works on it. A claim is the branch `claim/issue-<n>` on origin, holding one empty commit that names the agent and host. Git accepts only the first push of a new branch, so exactly one claimant wins. `scripts/issue.sh` does the whole cycle (bash; on Windows it runs under Git Bash):

```bash
scripts/issue.sh list                  # who holds what, on every machine, and for how long
scripts/issue.sh claim <n>             # claim and make the worktree ../Flint-<n>; exit 2: taken, pick another
scripts/issue.sh claim <n> --here      # claim for the worktree you're already in (a desktop-app worktree)
scripts/issue.sh claim <n> --base <b>  # branch from, and land on, the feature branch <b>; adds to either form above
scripts/issue.sh renew <n>             # at the start of each session on long work
scripts/issue.sh land <n>              # verify, push the branch and open a PR to main; the merge closes the issue and that releases the claim
scripts/issue.sh release <n>           # giving up: push your branch and comment where it stands first
scripts/issue.sh reap [--dry-run]      # delete the claims whose issue is closed; the SessionStart hook runs it
```

A claim ends when its issue closes: `.github/workflows/release-claim.yml` reaps it within a minute of the merge (or of any close), whichever machine held it, so nobody runs `release` after a merge.

- **A hook enforces it.** `.claude/settings.json` runs `scripts/issue-guard.sh` before every `git commit` and `git push`. It refuses issue work (a branch named `issue-<n>`, or a commit saying `Closes #<n>`) unless this session holds the claim, from the worktree it was claimed for. A chore with no issue (a typo, a dependency bump) needs no claim: keep it off `issue-<n>` branches and out of `Closes` lines. A branch built on a sibling's carries the sibling's `Closes` line; once that issue is closed (the sibling merged), the push passes without its claim. A SessionStart hook reaps the claims on closed issues, then lists the rest.
- **Only the session that claimed an issue renews, releases or lands it,** and only from its worktree. An existing worktree for an issue you didn't claim is another session's: leave it alone. Never delete another agent's `claim/*` branch, with one exception: a claim on a closed issue guards nothing, and `scripts/issue.sh reap` is the one way to clear it.
- **Work on a feature branch is claimed with `--base <branch>`,** never by changing `scripts/issue.conf`, which would ride into the default branch with the merge. The claim commit records the base, and `renew`, `--steal`, `land` and the hook all follow it. GitHub closes issues only for merges to the default branch, so merging the PR into the claim's base closes the issue, with a comment naming the PR and the branch, and releases the claim. Cut the feature branch from a default branch that already holds the release-claim workflow.
- **Claims go stale after 24 hours.** `claim <n> --steal` takes over a stale one; check the issue's comments and `origin/issue-<n>` for work in progress first, and continue from it. Taking a younger claim is Chris's call only: `claim <n> --steal --override "<his reason>"`.
- Settings for this repo are in `scripts/issue.conf`. The tool is vendored from `skills/setup-claims` in chrischaps/Dashboard: change it there, not here. `bash scripts/issue-test.sh` tests it against a throwaway remote.

<!-- claims:end -->
