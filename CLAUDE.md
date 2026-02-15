# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Flint is a CLI-first, AI-agent-optimized 3D game engine written in Rust. The core thesis inverts traditional engine design: the primary interface is CLI and code, with visual tools focused on *validating* results rather than *creating* them. Phase 5 complete: AI asset generation pipeline with pluggable providers (Flux textures, Meshy 3D models, ElevenLabs audio), style guides, semantic asset definitions, batch scene resolution, build manifests, model validation, and runtime catalog integration. Phase 4 complete with Rhai scripting, property/skeletal animation, spatial audio, interactable entities. PBR rendering from Phase 3, constraints from Phase 2. Game projects (e.g., the Doom-style FPS) live in their own repositories and include the engine as a git subtree. Extensible input system with TOML-based config, gamepad support (gilrs), layered loading, and runtime rebinding. GPU-instanced particle system with per-emitter pooling, alpha/additive blending, configurable emission shapes, and Rhai script control.

## Build & Development Commands

```bash
cargo build                    # Debug build
cargo build --release          # Release build
cargo run --bin flint -- <cmd> # Run CLI (e.g., cargo run --bin flint -- serve demo/showcase.scene.toml --watch)
cargo run --bin flint -- play demo/phase4_runtime.scene.toml  # Play a scene with first-person controls
cargo run --bin flint-player -- demo/phase4_runtime.scene.toml --schemas schemas  # Standalone player
cargo test                     # Run all tests (218 unit tests across crates)
cargo test -p flint-core       # Test a single crate
cargo clippy                   # Lint
cargo fmt --check              # Check formatting
```

## Development Workflow

Flint is designed for a **edit → validate → play** iteration loop. When making visual changes to scenes, models, or scripts, use these tools in order:

1. **`flint render`** — Headless scene-to-PNG snapshot. Use this to validate visual changes without launching a window. Fast, scriptable, and works in CI. This is the primary validation tool for AI agents.
   ```bash
   # Render a scene to a PNG file
   flint render levels/demo.scene.toml --output test.png --schemas schemas --width 1280 --height 720

   # Useful flags for framing the shot
   flint render levels/demo.scene.toml --output test.png --schemas schemas \
     --distance 20 --pitch 30 --yaw 45 --target 0,1,0 --no-grid --shadows
   ```

2. **`flint serve --watch`** — Interactive 3D viewer with hot-reload. Use for orbit-camera inspection and live scene editing.
   ```bash
   flint serve --watch levels/demo.scene.toml --schemas schemas
   ```

3. **`flint play`** — Full game runtime with physics, scripting, audio, and input. Use to test gameplay.
   ```bash
   flint play levels/demo.scene.toml --schemas schemas
   ```

**For AI agents:** After making changes to a scene file, model, or visual configuration, always render a snapshot with `flint render` to verify the result before moving on. The render command loads models (searches `scene_dir/models/` then parent `../models/`), generates spline geometry, and computes the full transform hierarchy — it produces output representative of what `flint play` will display.

## Architecture

19-crate Cargo workspace with clear dependency layering:

```
flint-cli           CLI binary (clap derive). Commands: init, entity, scene, query, schema, serve, play, validate, asset, render
  └── flint-asset-gen AI asset generation: pluggable providers (Flux, Meshy, ElevenLabs, Mock), style guides, batch resolution, validation, manifests
flint-player        Standalone player binary with game loop, physics, audio, animation, scripting, egui HUD, first-person controls
  ├── flint-script   Rhai scripting: ScriptEngine, ScriptSync, ScriptSystem (entity/input/audio/animation APIs, hot-reload)
  ├── flint-particles GPU-instanced particle system: ParticleSystem, ParticleSync, ParticlePool, emission shapes
  ├── flint-animation Property tween animation: AnimationClip, AnimationPlayer, AnimationSync, AnimationSystem
  ├── flint-audio    Kira spatial audio: AudioEngine, AudioSync, AudioTrigger, AudioSystem
  ├── flint-physics  Rapier 3D integration: PhysicsWorld, PhysicsSync, CharacterController
  ├── flint-runtime  Game loop infrastructure: GameClock, InputState (config-driven, multi-device), InputConfig, EventBus, RuntimeSystem trait
  ├── flint-viewer   egui-based GUI inspector
  ├── flint-import   glTF model importer
  ├── flint-asset    Content-addressed asset catalog
  ├── flint-constraint Constraint evaluation + auto-fix
  ├── flint-query    PEG query language (pest parser). Syntax: "entities where archetype == 'door'"
  ├── flint-scene    TOML scene serialization/deserialization with load/save
  ├── flint-render   wgpu-based PBR renderer (winit 0.30 ApplicationHandler, wgpu 23, orbit + first-person camera)
  ├── flint-ecs      hecs wrapper with stable EntityId mapping (BiMap), named entities, parent/child
  ├── flint-schema   Component/archetype schema system with TOML loading and runtime validation
  └── flint-core     Fundamental types: EntityId, ContentHash, Transform, Vec3, Color, FlintError
```

## Key Design Decisions

- **Dynamic components** stored as `toml::Value` (not Rust types) so archetypes are defined at runtime in schema TOML files
- **Stable EntityIds** via atomic counter (never recycled), persisted across save/load with counter adjustment to prevent collisions
- **Scene format is TOML** — human-readable, diffable, structured as `[scene]` metadata + `[entities.<name>]` sections
- **Schemas live in `schemas/`** directory — `components/*.toml` define field types and constraints, `archetypes/*.toml` bundle components with defaults. Multiple `--schemas` paths supported; later paths override earlier (game overrides engine)
- **Query language** uses pest PEG parser (`grammar.pest`). Supports: `==`, `!=`, `contains`, `>`, `<`, `>=`, `<=` on entity fields
- **Scene reload** is full re-parse (not incremental) to avoid borrow checker issues with `Mutex<ViewerState>`
- **Physics uses Rapier 3D** via `flint-physics` crate — kinematic character controller for player, static bodies for world geometry
- **Game loop** uses fixed-timestep accumulator pattern (1/60s default) for deterministic physics
- **Camera modes** — `CameraMode::Orbit` (scene viewer default) and `CameraMode::FirstPerson` (player) share the same view/projection math
- **Audio uses Kira** via `flint-audio` crate — spatial 3D audio with distance attenuation, non-spatial ambient loops, event-driven triggers
- **Animation system** via `flint-animation` crate — property tweens (Tier 1) with Step/Linear/CubicSpline interpolation, `.anim.toml` clip files; skeletal animation (Tier 2) with glTF skin/joint import, GPU vertex skinning, bone matrix computation, crossfade blending; `animator` + `skeleton` component schemas, plays in `update()` (variable-rate for smooth interpolation)
- **Scripting** via `flint-script` crate — Rhai scripting engine with entity/input/audio/animation/math/UI draw APIs; `script` component schema with `.rhai` files in `scripts/` directory; event callbacks (`on_init`, `on_update`, `on_collision`, `on_trigger_enter/exit`, `on_action`, `on_interact`, `on_draw_ui`); `on_update()` is parameterless — use `delta_time()` API for frame delta; load-time arity validation warns on mismatched callback signatures; hot-reload via file timestamp checking; ScriptCommand pattern for deferred audio/event effects; DrawCommand pattern for immediate-mode 2D UI rendering
- **Scriptable 2D UI overlay** — engine provides generic draw primitives (`draw_text`, `draw_rect`, `draw_circle`, `draw_line`, `draw_sprite` + `_ex` variants, `screen_width/height`, `measure_text`, `find_nearest_interactable`); scripts issue `DrawCommand`s each frame via `on_draw_ui()` callback; `PlayerApp` renders them via egui painter sorted by layer; sprites lazy-loaded from `sprites/` directory; no hardcoded HUD — all game UI is script-driven (e.g. `hud.rhai` for Doom FPS combat HUD, `hud_interact.rhai` for tavern interaction prompts)
- **Interactable system** — `interactable` component schema with prompt text, range, type, enabled flag; `find_nearest_interactable()` exposed to both Rust and Rhai scripts; interaction prompts rendered by game-level HUD scripts
- **Particle system** via `flint-particles` crate — GPU-instanced rendering with per-emitter pooled particles (up to 10,000); swap-remove pool for O(1) particle kill; CPU-side simulation (position, velocity, gravity, damping, size/color over lifetime); `ParticlePipeline` with alpha and additive blend modes; storage buffer for instance data; configurable emission shapes (point, sphere, cone, box); `particle_emitter` component schema; Rhai script API (`emit_burst`, `start_emitter`, `stop_emitter`, `set_emission_rate`); variable-rate update (not fixed-step); sprite sheet animation support
- **Billboard sprites** — camera-facing quads via `sprite` component; separate `BillboardPipeline` (not extending PBR); binary alpha via `discard` (avoids OIT); sprite sheet UV animation; per-sprite uniform buffer; renders after skinned entities
- **Raycasting** — Rapier `query_pipeline.cast_ray_and_get_normal()` exposed as `raycast()` in Rhai scripts; collider handle → EntityId resolution; exclude_collider for self-exclusion
- **Combat system** — game-level Rhai helper functions (`deal_damage()`, `get_health()`, `heal()`, `is_dead()`) defined in each game script that needs them, using generic `get_field`/`set_field` ECS access; engine has zero concept of "combat"; mouse button → action bindings for weapon firing
- **HUD overlay** — script-driven via `on_draw_ui()` callback + `DrawCommand` pipeline; engine renders 2D primitives (text, rect, circle, line, sprite) via egui painter; all game-specific UI (crosshair, health, ammo, damage flash, interaction prompts) lives in `.rhai` scripts, not engine code; `hud_controller` entity with `script.source = "hud.rhai"` pattern; supports layered rendering (negative layers = background, 0 = default, positive = foreground)
- **Game project pattern** — games live in their own repositories and include the engine as a git subtree at `engine/`; `--schemas` flag accepts multiple paths that merge with later-path-wins priority (e.g., `--schemas engine/schemas --schemas schemas`)
- **AI asset generation** via `flint-asset-gen` crate — pluggable `GenerationProvider` trait with Flux (textures), Meshy (3D models), ElevenLabs (audio), and Mock implementations; `StyleGuide` enriches prompts with palette/material/geometry constraints; `BatchStrategy` resolves entire scenes (AiGenerate, HumanTask, AiThenHuman); `BuildManifest` tracks provenance; `SemanticAssetDef` maps intent to generation requests; `validate_model()` checks GLB against style constraints
- **Asset config** — layered `FlintConfig`: `~/.flint/config.toml` < `.flint/config.toml` < env vars (`FLINT_{PROVIDER}_API_KEY`); per-provider API key/URL/enabled settings
- **Runtime catalog resolution** — `PlayerApp` optionally loads `AssetCatalog` + `ContentStore`; tries catalog name → hash → content store path before file-based fallback (backwards-compatible)
- **Input system** — config-driven, device-agnostic action bindings via `InputConfig` TOML files in `flint-runtime`; `Binding` enum unifies keyboard keys, mouse buttons, mouse delta/wheel, gamepad buttons, and gamepad axes (with deadzone, scale, invert, threshold, direction); layered loading: built-in defaults → game config → user overrides (`~/.flint/input_{game_id}.toml`) → CLI `--input-config` override; runtime rebinding via `rebind_action()` with Replace/Add/Swap modes and automatic user-override persistence; `ActionKind::Button` for discrete actions, `ActionKind::Axis1d` for analog values; gamepad support via `gilrs` 0.11 with `GamepadSelector::Any`/`Index(n)` for multi-controller; `actions_just_released()` now emits `GameEvent::ActionReleased`; scene TOML `[scene]` table supports optional `input_config` field

## Technical Gotchas

- `toml::toml!` macro produces `Map<String, Value>` not `Value` — needs `.into()` in tests
- wgpu v23 `Instance::new()` takes owned `InstanceDescriptor`, not a reference
- winit v0.30 uses `ApplicationHandler` trait with `run_app()` — no direct event loop matching
- `FlintError` uses `thiserror`; crate-level `Result<T>` aliases throughout
- Rapier v0.22 character controller types are in `rapier3d::control`, not `rapier3d::prelude`
- `DeviceEvent::MouseMotion` provides raw mouse delta for first-person camera (independent of cursor position)
- Rapier's `ChannelEventCollector` requires `crossbeam` channels for collision events
- Kira v0.11 uses `Decibels(f32)` tuple struct, not `from_amplitude`; `PlaybackRate(f64)` likewise
- Kira uses `glam::Vec3`/`glam::Quat` for listener/track positioning via `mint` interop
- `AudioManager::new()` may fail in headless/CI — always wrap in `Option` for graceful degradation
- Animation clips use `serde` derive on all types — `TrackTarget` uses `#[serde(tag = "type")]` for TOML compatibility
- `.anim.toml` files go in `demo/animations/` directory next to the scene; loaded by scanning the directory at startup
- Skeletal animation uses separate `SkinnedVertex` type (6 attributes) and `skinned_pipeline` to avoid overhead on static geometry
- Bone matrices stored in storage buffer (bind group 3) — uniform buffers have size limits; storage supports arbitrary bone counts
- `SkeletalSync` bridges ECS to skeletal playback; `skeleton` component + `animator` component on same entity triggers skeletal path
- Crossfade blending via `blend_target`/`blend_duration` fields on `animator` component — uses pose array slerp/lerp
- Rhai v1.24 with `sync` feature — `Arc<Mutex<ScriptCallContext>>` pattern for closure-captured world access
- ScriptCallContext uses raw `*mut FlintWorld` pointer — only valid during `call_update()`/`process_events()` scope
- `rhai::Engine::call_fn()` requires `&mut Scope` — per-entity Scopes preserve persistent script variables
- Callback detection via `ast.iter_functions()` checking function names — avoids calling non-existent functions
- `.rhai` files live in `scripts/` directory next to the scene; discovered via `script` component's `source` field
- Hot-reload checks file timestamps each frame; on recompile error, keeps old AST (never crashes)
- `on_interact` is sugar: fires on ActionPressed("interact") + proximity check; reads `interactable.range` (default 3.0) and `interactable.enabled` (default true)
- Entity IDs passed as `i64` in Rhai (native int type); cast to/from `EntityId` internally
- Animation control from scripts writes directly to `animator` component — `AnimationSync` picks up changes next frame
- `DrawCommand` enum in `flint-script::context` — pushed by Rhai draw API functions, drained per frame by `PlayerApp`
- `on_draw_ui()` callback runs after `on_update()` — scripts push `DrawCommand`s via `draw_text/rect/circle/line/sprite` functions
- Sprite textures lazy-loaded on first use via `image::open()` → `egui::ColorImage` → `egui::TextureHandle`; cached in `ui_textures` HashMap
- `measure_text()` uses approximate width (0.6 * font_size per char) — not exact but sufficient for layout
- `render_draw_commands()` is a free function (not method on PlayerApp) to avoid borrow conflicts with egui closures
- `load_pending_sprites()` must be called before `egui_winit` borrow in `render_hud()` to avoid double-mutable-borrow
- ureq v3 requires `json` feature for `send_json()`; uses rustls by default (no `tls-native-certs` feature)
- `GenerationProvider` trait's `generate()` takes `&Path` for output directory — providers write files directly
- `FlintConfig::load()` is layered: global `~/.flint/config.toml` merged with local `.flint/config.toml`, then env var overrides
- MockProvider generates minimal valid files (solid-color PNG, minimal GLB header, silence WAV) for testing without network
- `StyleGuide::find()` searches `styles/` then `.flint/styles/` for `{name}.style.toml`
- Model validation via `validate_model()` imports GLB through `import_gltf()` — reuses the same importer as the player
- `BuildManifest::from_assets_directory()` scans `.asset.toml` sidecars for `provider` property to identify generated assets
- `SemanticAssetDef` uses `#[serde(rename = "type")]` for `asset_type` field — TOML uses `type = "texture"` directly
- gilrs v0.11 `Gilrs::new()` may fail on headless/CI — wrapped in `Option` like `AudioManager`
- `ParticleInstance` and `ParticleInstanceGpu` have identical `#[repr(C)]` layouts (48 bytes, 3x `[f32; 4]`) — use `bytemuck::cast_slice` for zero-copy conversion between flint-particles and flint-render types
- Particle storage buffers: wgpu rejects zero-size buffers — skip draw calls for emitters with 0 alive particles
- Particle rendering: no depth write (`depth_write_enabled: false`), depth test still active — translucent particles don't occlude each other but hide behind walls
- Particle rendering order in `render_to()`: after billboard sprites, before wireframe overlay; alpha pass first, then additive pass
- `InputConfig` uses `BTreeMap` for deterministic serialization order; `Binding` uses `#[serde(tag = "type", rename_all = "snake_case")]` for TOML compatibility
- Gamepad button/axis names use gilrs `Debug` format (e.g., `"South"`, `"LeftStickX"`) — these are what `format!("{button:?}")` produces
- Gamepad axis deadzone uses linear remap: `(|value| - deadzone) / (1.0 - deadzone)`, applied before scale/invert
- `InputState` action evaluation: button actions treat any binding value >= 0.5 as pressed; axis1d sums all binding values
- User override persistence writes to `~/.flint/input_{game_id}.toml`, falling back to `.flint/input.user.toml` if home directory is unavailable
- Rebind capture mode (`pending_rebind` on PlayerApp) intercepts the next hardware event; gamepad axis capture requires magnitude >= 0.45 to distinguish from noise
- `InputConfig::from_toml_str()` validates key codes against winit `KeyCode` names and mouse buttons against Left/Right/Middle/Back/Forward

## Implemented vs Planned

**Working now:** Entity CRUD, scene load/save, query parsing/execution, schema introspection, PBR renderer with Cook-Torrance shading, glTF model import, cascaded shadow mapping, constraint validation + auto-fix, content-addressed asset catalog, egui GUI inspector, `serve --watch` hot-reload viewer, game loop with fixed-timestep physics, Rapier 3D character controller, first-person walkable scenes via `play` command, spatial audio with Kira (3D positioned sounds, ambient loops, event-driven triggers), property tween animation (Tier 1: keyframe clips with Step/Linear/CubicSpline, `.anim.toml` loading, `animator` component, event firing), skeletal animation (Tier 2: glTF skin/joint import, GPU vertex skinning via storage buffer, bone hierarchy computation, crossfade blending, skinned shadow mapping), Rhai scripting (entity/input/audio/animation/math/physics/combat/UI draw APIs, event callbacks incl. `on_draw_ui`, hot-reload), **scriptable 2D UI overlay** (DrawCommand pipeline with text/rect/circle/line/sprite primitives, layer sorting, lazy sprite loading, `hud.rhai` for game-specific HUD), interactable entities with script-driven prompts, NPC behavior scripts (bartender/patron/stranger), footstep sounds, ambient events, full atmospheric tavern integration demo, AI asset generation pipeline (Flux textures, Meshy 3D models, ElevenLabs audio with mock provider for testing, style guides, batch scene resolution, model validation, build manifests, semantic asset definitions, runtime catalog integration), **Doom-style FPS** (Phase A: billboard sprite rendering, hitscan raycast weapons, health/damage system, enemy AI state machine, health/ammo pickups, combat HUD with damage flash, mouse button action bindings, multi-directory schema loading for game/engine separation), **extensible input system** (TOML-based InputConfig with layered loading, device-agnostic Binding enum for keyboard/mouse/gamepad, gilrs gamepad integration, runtime rebinding with Replace/Add/Swap modes, user-override persistence, ActionKind Button/Axis1d, `--input-config` CLI flag, scene-level input_config field), **GPU-instanced particle system** (per-emitter pooling with swap-remove, alpha/additive blend pipelines, configurable emission shapes (point/sphere/cone/box), size/color over lifetime, gravity/damping, sprite sheet animation, Rhai script API with `emit_burst`/`start_emitter`/`stop_emitter`/`set_emission_rate`, `particle_emitter` component schema, variable-rate CPU simulation with GPU storage buffer rendering).

**Designed but not implemented:** See `flint-design-doc.md` for full specification of remaining phases.

## Project Structure

- `crates/` — All 18 workspace crates
- `schemas/` — Engine component and archetype TOML definitions (transform, door, bounds, material, rigidbody, collider, character_controller, player, audio_source, audio_listener, audio_trigger, animator, skeleton, script, interactable, sprite, particle_emitter, asset_def, etc.)
- Game projects live in separate repositories with the engine as a git subtree (e.g., the Doom FPS)
- `styles/` — Style guide TOML definitions (medieval_tavern) for AI generation
- `demo/` — Showcase scenes (showcase, phase3_showcase, phase4_runtime, phase5_ai_demo), demo `scripts/` (.rhai), `audio/` assets, and `animations/` clips
- `testGame/` — Test project directory (levels/, schemas/)
- `flint-design-doc.md` — Comprehensive design document covering all planned phases
- `DOOM_FPS_GAPS.md` — moved to the Doom FPS game repository
- `PHASE4_ROADMAP.md` — Phase 4 status tracker (all 5 stages complete)
- `PHASE5_ROADMAP.md` — Phase 5 status tracker (all 5 stages complete)
