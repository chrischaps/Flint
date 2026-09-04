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
  │                 validate-suite, play-suite, calibrate, play-chart, replay-chart, render-suite, spike-rumble
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
  ├── flint-physics  Rapier 3D (character controller, static bodies)
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
  └── flint-core     EntityId, ContentHash, Transform, Vec3, Color, FlintError
```

## Key Design Principles

- **Dynamic components** stored as `toml::Value` -- archetypes defined at runtime in schema TOML files, not Rust types
- **Stable EntityIds** via atomic counter (never recycled), persisted across save/load
- **Scene format is TOML** -- `[scene]` metadata + `[entities.<name>]` sections; reload is full re-parse
- **Schemas in `schemas/`** -- `components/*.toml` for field types, `archetypes/*.toml` for bundles. Multiple `--schemas` paths; later overrides earlier (game overrides engine)
- **Scripting via Rhai** -- `script` component with `.rhai` files in `scripts/`; callbacks: `on_init`, `on_update`, `on_collision`, `on_trigger_enter/exit`, `on_action`, `on_interact`, `on_draw_ui`, `on_scene_exit/enter`, `on_animation_end`; `ScriptCommand` for deferred effects; `DrawCommand` for immediate-mode 2D UI
- **Particles** -- inline `particle_emitter` or reusable `particles/*.particles.toml` effects via `particle_effect` (multi-emitter, curves, forces, bursts, sub-emitters; ADR 0068). Deterministic: per-emitter seeds from entity *names*, fixed-step `simulate_to`, `flint render --particle-time`. One shared `ParticleInstance` type; every consumer uploads through `SceneRenderer::update_particles_from` after `ParticleSystem::pack`. Scripts: `play_effect`/`stop_effect`/`set_effect_param`. F3 Particles panel in the player; F4 Particles section in the viewer
- **All game UI is script-driven** -- no hardcoded HUD; `hud_controller` entity + `hud.rhai` pattern; engine provides draw primitives (`draw_text/rect/circle/line/sprite`), scripts compose UI
- **Game project pattern** -- games in own repos with engine as git subtree at `engine/`; `--schemas engine/schemas --schemas schemas` for layered overrides
- **Fixed-timestep physics** (1/60s) via accumulator; animation/particles run at variable rate
- **Post-processing** -- HDR pipeline (`Rgba16Float`) with bloom, SSAO, fog, volumetric (god rays), Kuwahara (anisotropic painterly filter), vignette; `[post_process]` TOML block; F4 opens the Rendering & Effects debug menu in the player and viewer (all toggles + parameters, ADR 0053); when active, PBR shaders output linear HDR
- **Scene transitions** -- `TransitionPhase` lifecycle with script-driven visuals; `PersistentStore` survives transitions; `GameStateMachine` pushdown automaton with per-system `SystemPolicy`
- **Input system** -- TOML-based `InputConfig` with layered loading (built-in -> game -> user overrides -> CLI); keyboard/mouse/gamepad/touch unified via `Binding` enum

## Critical Gotchas

- `toml::toml!` macro produces `Map<String, Value>` not `Value` -- needs `.into()` in tests
- wgpu v23: `Instance::new()` takes owned `InstanceDescriptor`; max 4 bind groups (0-3); `wgpu::Queue` is not `Clone` -- pass `&Queue` through, do not stash one in a struct
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
