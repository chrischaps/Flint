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
cargo run --bin flint -- play demo/phase4_runtime.scene.toml  # Play a scene
cargo run --bin flint-player -- demo/phase4_runtime.scene.toml --schemas schemas  # Standalone player
cargo run --bin flint -- export web demo/  # any game as a folder a static server plays (#106):
                               #   demo/dist/web = player + games/<id>/ bundles + games.json;
                               #   --out, --player <trunk dist> (else trunk builds it), --meta k=v, --clean
cargo test                     # Run all tests
cargo test -p flint-core       # Test a single crate
cargo clippy                   # Lint
cargo fmt --all                # Format (rustfmt.toml pins the edition; `rustfmt <file>` for one file)
bash scripts/ci.sh             # What a PR must pass: formatting, in seconds. `issue.sh land` runs it.
bash scripts/ci.sh --full      #   ...plus what CI's clippy and wasm jobs run (two builds)
```

## Development Workflow

Flint uses an **edit -> validate -> play** loop:

1. **`flint render`** -- Headless scene-to-PNG snapshot. Primary validation tool for AI agents.
   ```bash
   flint render levels/demo.scene.toml --output test.png --schemas schemas --width 1280 --height 720
   flint render levels/demo.scene.toml --output test.png --schemas schemas \
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
   ```
   More, and the goldens: `crates/flint-cli/CLAUDE.md`.

4. **`flint play`** -- Full game runtime with physics, scripting, audio, and input.
   ```bash
   flint play demo/phase4_runtime.scene.toml
   flint play demo/phase4_runtime.scene.toml --width 1920 --height 1080
   ```
   More: `crates/flint-player/CLAUDE.md`.

**For AI agents:** After visual changes, always run `flint render` to verify before moving on. The render command loads models (searches `scene_dir/models/` then `../models/`), generates geometry, and computes the full transform hierarchy.

## Architecture

26-crate Cargo workspace (plus `tools/arch-analyzer`, a syn-based dependency/metrics extractor feeding the `tools/arch-viewer` web graph and the book's crate table, checked in CI; neither is a default member):

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
  ├── flint-debug-ui DebugPanel trait + F3/F4 panel roster (Rendering & Effects, ocean, sky, camera, grass, particles, Living Props...); shared `widgets` (drag helpers, CurveEditor, GradientEditor); optional via debug-hud
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
| `tools/showcase/CLAUDE.md` | film rig |
| `docs/CLAUDE.md` | book build and publish |

A bullet below that names a feature in a line or two has its full text in one of these.

## Key Design Principles

- **Dynamic components** stored as `toml::Value` -- archetypes defined at runtime in schema TOML files, not Rust types
- **Stable EntityIds** via atomic counter (never recycled), persisted across save/load
- **Scene format is TOML** -- `[scene]` metadata + `[entities.<name>]` sections; reload is full re-parse
- **Schemas in `schemas/`** -- `components/*.toml` for field types, `archetypes/*.toml` for bundles. Several directories load in order, later overriding earlier (game overrides engine). One resolver names them for every command, the viewer and the player (ADR 0114; `crates/flint-core/CLAUDE.md`): repeatable `--schemas` flags if given (the whole list), else `[project] schemas` in the project's `flint.toml`, else `schemas/` in the working directory, beside the scene and one level up
- **Scripting via Rhai** -- `script` component with `.rhai` files in `scripts/`; callbacks: `on_init`, `on_update`, `on_collision`, `on_trigger_enter/exit`, `on_action`, `on_interact`, `on_draw_ui`, `on_scene_exit/enter`, `on_animation_end`, `on_sequence_cue`, `on_event`, `on_expression(state)`; `ScriptCommand` for deferred effects; `DrawCommand` for immediate-mode 2D UI
- **All game UI is script-driven** -- no hardcoded HUD; `hud_controller` entity + `hud.rhai` pattern; engine provides draw primitives (`draw_text/rect/circle/line/sprite`), scripts compose UI
- **Game project pattern** -- games in own repos with engine as git subtree at `engine/`; `schemas = ["engine/schemas", "schemas"]` under `[project]` in the game's `flint.toml` layers the game's schemas over the engine's with no flags, and `--schemas engine/schemas --schemas schemas` overrides it for a run
- **Player extensions** (ADRs 0071-0075) -- a game binary embeds `PlayerApp` and installs `PlayerExtension`s (`add_extension`): native Rhai fns, `pre_update` before scripts' `on_update`, window events, egui UI. Book: guides/player-extensions.md
- **Every runtime read goes through `flint_core::vfs`** (#97) -- `vfs::read` / `read_to_string` / `read_dir` / `exists` / `is_file` / `is_dir`, never `std::fs` or `Path::exists` in a runtime crate. Decode images with `flint_render::open_image` and glTF through `flint_import::import_gltf`. Tests and the CLI keep `std::fs`
- **Fixed-timestep physics** (1/60s) via accumulator; animation/particles run at variable rate

## Critical Gotchas

- `toml::toml!` macro produces `Map<String, Value>` not `Value` -- needs `.into()` in tests
- wgpu v23: `Instance::new()` takes owned `InstanceDescriptor`; max 4 bind groups (0-3); `wgpu::Queue` is not `Clone` -- pass `&Queue` through, do not stash one in a struct
- winit v0.30: `ApplicationHandler` trait with `run_app()`; `DeviceEvent::MouseMotion` for raw mouse delta
- Rapier v0.22: character controller types in `rapier3d::control`, NOT `rapier3d::prelude`
- Kira v0.11: `Decibels(f32)` and `PlaybackRate(f64)` are tuple structs; uses `glam` via `mint`
- `AudioManager::new()` / `Gilrs::new()` may fail in headless/CI -- wrap in `Option`
- Rhai v1.24 `sync`: `call_fn()` Scope visible only to direct callee, NOT sub-functions; entity IDs as `i64`; no implicit numeric coercion (`0.0` is NOT `0`); draw `_ex` functions take `layer` as int (`0`), not float (`0.0`)
- `flint-android` excluded from `default-members` -- use `cargo ndk -p flint-android`; min API 26. `cargo test --workspace` fails on `ndk-sys` on Windows; use plain `cargo test`
- Windows links a 1 MB main-thread stack; debug builds of flint-cli's clap parser overflow it -- `.cargo/config.toml` links with `/STACK:8388608`. Don't remove it, and don't grow `Commands` assuming stack is free.
- Scene entity `merge_component()` does field-level merge INTO archetype defaults, not full replacement
- `flint_core::toml_util` -- use `toml_f64`/`toml_f32`/`toml_vec3`/etc. instead of inline coercion patterns
- Never use `archetype = "furniture"` on non-visual entities -- furniture includes `bounds`, renders teal boxes
- Per-frame GPU allocations are the renderer's CPU cliff: never `create_buffer`/`create_bind_group` inside a per-entity loop; what a draw needs per entity comes from a cache (`crates/flint-render/CLAUDE.md`)
- One submission a frame (#139): `SceneRenderer::encode_to` records the whole frame (grass, shadows, scene, post-process, depth upscale, world lines) into the caller's encoder, the player adds its HUD and the GPU timer's resolve (`encode_gpu_timings`), submits once, then calls `frame_submitted` (grass and timer read-backs). `render_to` is that with its own submit.
- CI (`.github/workflows/ci.yml`, #757) runs `rustfmt` (the tree is formatted; an unformatted file fails the PR and `issue.sh land`), `clippy (native)` (`cargo clippy --all-targets`: warnings pass, a deny-level lint or code that doesn't compile fails), the wasm and headless checks, and the GPU tests on WARP, which don't block while they fail on main (`continue-on-error`). No unit tests run in CI yet
- CI's `wasm32 check` (`.github/workflows/ci.yml`) builds `flint-player` and every crate it links for `wasm32-unknown-unknown` on each PR. Run it locally with `bash scripts/wasm-check.sh` before pushing runtime-crate changes. It also denies `Instant::now`/`SystemTime::now` and their `elapsed()`, which compile on wasm but panic there: runtime code uses the `web-time` crate (a workspace dep: `web-time = { workspace = true }`), which #100 moved every older site to. Desktop-only deps in a runtime crate go under `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]`, as in flint-player's Cargo.toml (docs/design/web.md)
- `FlintWorld::spawn` ids come from a process-wide counter and scenes load from a `HashMap`, so entity ids differ run to run -- derive anything reproducible (seeds) from entity **names**, never ids
- Lights are `[entities.x.light]` with a `type`, and a directional `direction` points *toward* the light. No system reads a `directional_light` table and `flint validate` passes it, so a scene with only that gets `default_scene_lights` and no warning (several benches do).

## Documentation Pipeline

`CHANGELOG.md` is for games that carry the engine as a subtree (#765): a change a game can notice (a scene or schema key, a script function, a CLI flag, a format, a Cargo feature, a change in look or behaviour) gets an entry saying what changed and what to do. CI's `changelog` job (`scripts/changelog-check.sh`) fails a PR that touches `schemas/`, the script API or the scene format with no entry; a commit-message line `Changelog: none (reason)` waives it for a change no game can notice.

The book is `docs/book` (mdBook), published to docs.chaps.dev by hand from this machine. There is no drift checker and no CI for the docs: when a change alters a command, flag, key or component the book describes, update the book in the same PR, checking against the code rather than memory.

Build, preview and publish commands: `docs/CLAUDE.md`.

## Project Structure

- `crates/` -- All workspace crates
- `schemas/` -- Engine component and archetype TOML definitions
- `styles/` -- Style guide TOML definitions for AI generation
- `demo/` -- Showcase scenes, scripts, audio, animations, `particles/` effect assets
- `testGame/` -- Test project (levels/, schemas/)
- `docs/book/` -- mdBook source; `docs/build.ps1` / `docs/publish.sh` -- build, preview and publish it
- `docs/design/flint-design-doc.md` -- Full design document for remaining phases
- `docs/adr/` -- Architecture decision records from ADR 0111 on (earlier numbers live only in commit messages); `README.md` says how to take a number

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
