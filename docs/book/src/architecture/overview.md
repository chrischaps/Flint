# Architecture Overview

Flint is structured as a twenty-seven-member Cargo workspace (twenty-six crates under `crates/` plus the `tools/arch-analyzer` binary) with clear dependency layering. Each crate has a focused responsibility, and dependencies flow in one direction --- from the binaries down to core types.

## Workspace Structure

```
flint/
├── crates/
│   ├── flint-cli/          # CLI binary (clap). Entry point for all commands.
│   ├── flint-asset-gen/    # AI asset generation: providers, style guides, batch resolution
│   ├── flint-procgen/      # Procedural generation: Generator trait, registry, tree/texture/creature generators
│   ├── flint-procgen-ai/   # AI-assisted procgen: ProcGenAgent trait, spec creation/refinement
│   ├── flint-player/       # Standalone player binary with game loop, physics, audio, animation, scripting
│   ├── flint-android/      # Android entry point (NativeActivity, APK asset extraction)
│   ├── flint-script/       # Rhai scripting: ScriptEngine, ScriptSync, hot-reload
│   ├── flint-viewer/       # egui-based GUI inspector with hot-reload
│   ├── flint-debug-ui/     # DebugPanel trait + the F3/F4 panel roster (Rendering & Effects, ocean, sky, ...)
│   ├── flint-music/        # Rhythm sessions: suite manifests, charts, tempo maps, ladder, seam, replay
│   ├── flint-input-capture/# 1 kHz gamepad capture thread stamped against the audio clock
│   ├── flint-particles/    # GPU-instanced particle system with pooling and emission shapes
│   ├── flint-animation/    # Two-tier animation: property tweens + skeletal/glTF
│   ├── flint-audio/        # Kira spatial audio: 3D sounds, ambient loops, triggers
│   ├── flint-terrain/      # Heightmap terrain with splat-map blending
│   ├── flint-runtime/      # Game loop infrastructure (GameClock, InputState, EventBus, GameStateMachine)
│   ├── flint-physics/      # Rapier 3D integration (PhysicsWorld, CharacterController)
│   ├── flint-render/       # wgpu PBR renderer with Cook-Torrance shading + skinned mesh pipeline
│   ├── flint-import/       # File importers (glTF/GLB with skeleton/skin extraction)
│   ├── flint-asset/        # Content-addressed asset storage and catalog
│   ├── flint-constraint/   # Constraint definitions and validation engine
│   ├── flint-query/        # PEG query language (pest parser)
│   ├── flint-scene/        # TOML scene serialization/deserialization
│   ├── flint-ecs/          # hecs wrapper with stable IDs, names, hierarchy
│   ├── flint-schema/       # Component/archetype schema loading and validation
│   └── flint-core/         # Fundamental types: EntityId, Transform, Vec3, etc.
├── tools/
│   ├── arch-analyzer/      # syn-based workspace analyzer that emits crate/dependency/metrics JSON
│   └── arch-viewer/        # Static web page that renders that JSON as an interactive graph
├── schemas/                # Default component, archetype, and constraint definitions
├── demo/                   # Showcase scenes and build scripts
└── docs/                   # This documentation (mdBook)
```

## Design Decisions

### Dynamic Components

The most significant architectural choice: components are stored as `toml::Value` rather than Rust types. This means:

- **Archetypes are runtime data**, not compiled types
- New components can be defined in TOML without recompiling
- The schema system validates component data against definitions
- Trade-off: less compile-time safety, more flexibility

### Stable Entity IDs

Entity IDs are monotonically increasing 64-bit integers that never recycle. A `BiMap` maintains the mapping between `EntityId` and hecs `Entity` handles. On scene load, the ID counter adjusts to be above the maximum existing ID.

### Scene as Source of Truth

The TOML file on disk is canonical. In-memory state is derived from it. The `flint edit` viewer (with `--watch`) re-parses the entire file on change rather than attempting incremental updates. This is simpler and avoids synchronization bugs.

### Fixed-Timestep Physics

The game loop uses a fixed-timestep accumulator pattern (1/60s default). Physics simulation steps at a constant rate regardless of frame rate, ensuring deterministic behavior. Rendering interpolates between physics states for smooth visuals.

### Error Handling

All crates use `thiserror` for error types. Each crate defines its own error enum and a `Result<T>` type alias. Errors propagate upward through the crate hierarchy.

## Technology Choices

| Component | Technology | Rationale |
|-----------|------------|-----------|
| Language | Rust | Performance, safety, game ecosystem |
| ECS | hecs | Lightweight, standalone, well-tested |
| Rendering | wgpu 23 | Cross-platform, modern GPU API |
| Windowing | winit 0.30 | `ApplicationHandler` trait pattern |
| Physics | Rapier 3D 0.22 | Mature Rust physics, character controller |
| Audio | Kira 0.11 | Rust-native, game-focused, spatial audio |
| GUI | egui 0.30 | Immediate-mode, easy integration with wgpu; also drives the debug panels and player HUD |
| Gamepads | gilrs + gilrs-core | Frame-rate polling in the player; direct 1 kHz polling and rumble in `flint-input-capture` |
| Audio decoding | symphonia, hound | Stem decoding and WAV output for offline suite renders in `flint-music` |
| Noise / RNG | noise, rand, rand_chacha | Deterministic procgen and terrain generation |
| Node editor | egui-snarl | The texture pipeline editor inside `flint edit` |
| Native dialogs | rfd | File pickers in the editors |
| Scene format | TOML | Human-readable, diffable, good Rust support |
| Query parser | pest | PEG grammar, good error messages |
| Scripting | Rhai 1.24 | Sandboxed, embeddable, Rust-native |
| AI generation | ureq | Lightweight HTTP client for provider APIs |
| CLI framework | clap (derive) | Ergonomic, well-documented |
| Error handling | thiserror + anyhow | Typed errors in libraries, flexible in binary |

## Data Flow

Flint has two entry points: the CLI for scene authoring and validation, and the player for interactive gameplay. Both flow through the same crate hierarchy:

```
User / AI Agent
      │
      ├──────────────────────────────────┐
      ▼                                  ▼
  flint-cli                        flint-player
  (scene authoring, rhythm tools)  (interactive gameplay)
      │                                  │
      ├──► flint-viewer    (GUI)         ├──► flint-runtime   (game loop, input)
      ├──► flint-query     (queries)     ├──► flint-physics   (Rapier 3D)
      ├──► flint-scene     (load/save)   ├──► flint-audio     (Kira spatial audio)
      ├──► flint-render    (renderer)    ├──► flint-music     (rhythm sessions)
      ├──► flint-constraint(validation)  ├──► flint-input-capture (1 kHz gamepad)
      ├──► flint-asset     (catalog)     ├──► flint-animation (tweens + skeletal)
      ├──► flint-asset-gen (AI gen)      ├──► flint-particles (GPU particles)
      ├──► flint-procgen   (proc gen)    ├──► flint-script    (Rhai scripting)
      ├──► flint-music     (suite tools) ├──► flint-terrain   (heightmap terrain)
      ├──► flint-input-capture           ├──► flint-debug-ui  (F3/F4 panels, optional)
      ├──► flint-player    (play)        └──► flint-render    (PBR + skinned mesh)
      └──► flint-import    (glTF import)         │
              │                                  ▼
              ▼                              flint-import  (glTF meshes + skins)
          flint-ecs                              │
          flint-schema                           ▼
          flint-core                         flint-ecs
                                             flint-schema
                                             flint-core
```

`flint-cli` depends on `flint-player` directly: `flint play` is the player embedded in the CLI, and the rhythm commands (`play-chart`, `replay-chart`, `render-suite`, ...) drive `flint-music` and `flint-input-capture` without a scene at all. `flint-procgen-ai` is tool-time only and is not a default workspace member.

## Crate Details

### flint-core

Fundamental types shared by all crates. Minimal external dependencies (`thiserror`, `serde`, `sha2`).

- `EntityId` --- stable 64-bit entity identifier
- `ContentHash` --- SHA-256 based content addressing
- `Transform`, `Vec3`, `Color` --- geometric primitives
- `FlintError` --- base error type

### flint-schema

Loads component and archetype definitions from TOML files. Provides a registry for introspection. Supports field types (`bool`, `i32`, `i64`, `f32`, `f64`, `string`, `vec2`, `vec3`, `vec4`, `color`, `transform`, `enum`, `entity_ref`) with validation constraints. Since the scene loader started applying field defaults and validating on load, the registry is consulted at runtime as well as by `flint validate`.

### flint-ecs

Wraps hecs with:
- `BiMap<EntityId, hecs::Entity>` for stable ID mapping
- Named entity lookup
- Parent-child relationship tracking
- Atomic ID counter for deterministic allocation

### flint-scene

TOML serialization and deserialization for scenes. Handles the mapping between on-disk format and in-memory ECS world.

### flint-query

PEG parser (pest) for the query language. Parses queries like `entities where archetype == 'door'` and executes them against the ECS world.

Supported operators: `==`, `!=`, `>`, `<`, `>=`, `<=`, `contains`

### flint-constraint

Constraint engine that validates scenes against declarative TOML rules. Supports required components, value ranges, reference validity, and custom query rules. Includes an auto-fix system with cascade detection.

### flint-asset

Content-addressed asset storage with SHA-256 hashing. Manages an asset catalog with name/hash/type/tag indexing. Supports resolution strategies (strict, placeholder).

### flint-import

File importers for bringing external assets into the content-addressed store. Supports glTF/GLB with mesh, material, and texture extraction.

### flint-render

wgpu 23 PBR renderer with:
- **Cook-Torrance shading** --- physically-based BRDF with roughness/metallic workflow
- **Cascaded shadow mapping** --- directional light shadows across multiple distance ranges
- **glTF mesh rendering** --- imported models rendered with full material support
- **Billboard sprite pipeline** --- camera-facing quads with sprite sheet animation and binary alpha
- **Camera modes** --- orbit (scene viewer) and first-person (player), sharing view/projection math
- **Headless mode** --- render to PNG for CI and automated screenshots

### flint-viewer

egui-based GUI inspector built on top of `flint-render`:
- Entity tree with selection, transform gizmos, undo/redo, TOML write-back
- Component property editor
- Constraint violation overlay
- Hot-reload via file watching (`flint edit --watch`)
- Hosts the `flint-debug-ui` panels, including the F4 Rendering & Effects menu

### flint-debug-ui

The summonable debug overlay shared by the viewer and the player:
- `DebugPanel` trait (`name`, `ui`, `is_open`, `toggle`, `layout`, dirty tracking) and `PanelLayout::{SideRight, Bottom}`
- `assign_columns()` balances open panels across three side columns by a per-panel weight
- Panel roster: Rendering & Effects (`RENDER_DEBUG_PANEL`, F4), Ocean, Day / Time, Camera, Grass, Reality, Weather, Visitor, Dead Calm
- Depends on `flint-render` (it mirrors and writes through to renderer state), `flint-terrain`, and `flint-scene`
- Optional in the player behind the default-on `debug-hud` cargo feature; feature-off builds carry no debug surface

### flint-runtime

Game loop infrastructure for interactive scenes:
- `GameClock` --- fixed-timestep accumulator (1/60s default)
- `InputState` and `InputConfig` --- keyboard/mouse/gamepad tracking with TOML-configured action bindings
- `EventBus` --- decoupled event dispatch between systems
- `RuntimeSystem` trait --- standard interface for update/render systems
- `GameStateMachine` --- pushdown automaton for game states (play, pause, menu) with per-system `SystemPolicy`
- `PersistentStore` --- key-value data that survives scene transitions

### flint-physics

Rapier 3D integration:
- `PhysicsWorld` --- manages Rapier rigid body and collider sets, raycasting via `EntityRaycastHit`
- `PhysicsSync` --- bridges TOML component data to Rapier bodies, maintains collider-to-entity mapping
- `CharacterController` --- kinematic first-person movement with gravity, jumping, and ground detection
- Uses kinematic bodies for player control, static bodies for world geometry

### flint-audio

Kira 0.11 integration for game audio:
- `AudioEngine` --- wraps Kira AudioManager, handles sound loading and listener positioning
- `AudioSync` --- bridges TOML `audio_source` components to Kira spatial tracks
- `AudioTrigger` --- maps game events (collision, interaction) to sound playback
- Spatial 3D audio with distance attenuation, non-spatial ambient loops
- Graceful degradation when no audio device is available (headless/CI)

### flint-music

Data contract and runtime for rhythm-driven games ("linear composition, adaptive playback"). Originated in Starchild, engine-generic:
- `SuiteManifest` (tempo map, sections, re-entry points, a fixed six-bus stem inventory) and `Chart` (continuous input curves, discrete pulses, scene cues), with `validate_manifest` / `validate_chart` cross-checking both against the stems on disk
- `Conductor`, `TempoMap`, `MusicalPosition` --- beat/bar arithmetic and the `ClockBridge` that maps the audio clock onto game time
- `ChartSession` / `ChartCore` --- the per-frame judgment loop producing `ConductedFrame` values that scripts read through the `conducted_*` API
- `Coherence`, `Judge`, `Ladder` / `LadderDriver`, `Reintegrator` --- the disintegration ladder, hysteresis, and the seam that brings the ensemble back in
- `GradientDriver`, `HapticsDriver`, `BusMixer` --- error-driven audio gradient, rumble entrainment, six-bus stem mixing
- `SessionWriter` / `read_session` / `synthesize` --- `.session.jsonl` recording, replay, and synthetic input profiles
- `render_offline` / `write_wav` --- headless suite rendering behind `flint render-suite`
- Depends only on `flint-core` plus kira, symphonia, hound

### flint-input-capture

A dedicated thread that owns `Gilrs` and polls it at 1 kHz (`CaptureConfig::poll_hz`), stamping each event with a latency-compensated suite sample from the `ClockBridge` and emitting `flint-music` `InputEvent`s over a channel:
- `VerbMap::{Prototype, Full}` maps sticks, buttons and triggers onto the chart verb space (`lean`, `sway`, `pulse`, `press`, `flick`, `pressure_l/r`); charts never see buttons
- `measure_granularity` reports the driver's real poll cadence; `rumble` drives haptics directly over XInput
- Uses the gilrs XInput backend on Windows because the default WGI backend delivers nothing to console apps
- Depends on `flint-core` and `flint-music`

### flint-animation

Two-tier animation system:
- **Tier 1: Property tweens** --- `AnimationClip` with keyframe tracks targeting transform properties (position, rotation, scale) or custom fields. Step, Linear, and CubicSpline interpolation. Clips defined in `.anim.toml` files.
- **Tier 2: Skeletal animation** --- `Skeleton` and `SkeletalClip` types for glTF skin/joint hierarchies. GPU vertex skinning via bone matrix storage buffer. Crossfade blending between clips.
- `AnimationSync` bridges ECS `animator` components to property playback
- `SkeletalSync` bridges ECS to skeletal playback with bone matrix computation
- Layer stack (additive / override, per-joint masks, weight fades) and timed `.sequence.toml` playback with `on_sequence_cue` callbacks
- glTF `CUBICSPLINE` samplers import with tangents and hemisphere-continuous quaternion keys

### flint-particles

GPU-instanced particle system for visual effects:
- **ParticlePool** --- swap-remove array for O(1) particle death, contiguous alive iteration
- **ParticleSync** --- bridges ECS `particle_emitter` components to the simulation, auto-discovers new emitters each frame
- **ParticleSystem** --- top-level `RuntimeSystem` that ticks simulation in `update()` (variable-rate, not fixed-step)
- **ParticlePipeline** --- wgpu render pipeline with alpha and additive variants, storage buffer for instances
- Emission shapes: point, sphere, cone, box. Value-over-lifetime interpolation for size and color.

### flint-script

Rhai scripting engine for runtime game logic:
- `ScriptEngine` --- compiles `.rhai` files, manages per-entity `Scope` and `AST`, dispatches callbacks
- `ScriptSync` --- discovers entities with `script` components, monitors file timestamps for hot-reload
- `ScriptSystem` --- `RuntimeSystem` implementation running in `update()` (variable-rate)
- Full API: entity CRUD, input, time, audio, animation, physics (raycast, camera), math, events, logging, UI draw
- `ScriptCommand` pattern --- deferred audio/event effects processed by PlayerApp after script batch
- `DrawCommand` pattern --- immediate-mode 2D draw primitives (text, rect, circle, line, sprite) rendered via egui
- `ScriptCallContext` with raw `*mut FlintWorld` pointer for world access during call batches
- Depends on `flint-physics` for raycast and camera direction access

### flint-asset-gen

AI asset generation pipeline:
- `GenerationProvider` trait with pluggable implementations (Flux, Meshy, ElevenLabs, Mock)
- `StyleGuide` --- TOML-defined visual vocabulary (palette, materials, geometry constraints) for prompt enrichment
- `SemanticAssetDef` --- maps intent (description, material, wear level) to generation requests
- Batch scene resolution with strategies: `AiGenerate`, `HumanTask`, `AiThenHuman`
- `validate_model()` --- checks GLB geometry and materials against style constraints
- `BuildManifest` --- provenance tracking (provider, prompt, content hash) for all generated assets
- `FlintConfig` --- layered configuration for API keys and provider settings
- `JobStore` --- persistent tracking of async generation jobs (for long-running 3D model generation)

### flint-procgen

Procedural generation framework:
- `Generator` trait --- pluggable generator interface with `generate()`, `param_schema()`, `estimate_cost()`
- `GeneratorRegistry` --- register and look up generators by type name
- `ProcGenSpec` --- TOML spec format with metadata, seed config, and generator parameters
- Built-in generators: `tree_v1` (L-system/space colonization trees), `texture_v1` (PBR texture maps), `creature_v1`
- `ProcGenCache` --- LRU cache keyed by (spec_hash, seed) with memory budget
- Algorithmic building blocks: noise (Perlin, simplex, Worley, FBM), L-system engine, mesh builder, space colonization

### flint-procgen-ai

AI-assisted procedural generation (tool-time only):
- `ProcGenAgent` trait --- `interpret_spec()`, `create_spec_from_prompt()`, `refine_spec()`
- `MockAgent` implementation for testing

### flint-terrain

Heightmap terrain system:
- Chunked mesh generation from grayscale PNG heightmaps
- RGBA splat-map blending for up to 4 texture layers
- Bilinear height interpolation for smooth surfaces
- `terrain_height(x, z)` callback for script queries
- Does NOT depend on `flint-render`; uses standard `Vertex` format

### flint-android

Android entry point (excluded from default workspace members):
- `NativeActivity` integration
- APK asset extraction
- Build via `cargo ndk -p flint-android`; min API 26

### flint-player

Standalone player binary that wires together runtime, physics, audio, animation, particles, scripting, music sessions, and rendering. The `player_app` module is decomposed into named lifecycle files (ADR 0062): `init` (construction and loading), `frame` (the per-frame loop), `events` (window and key handling), `transition` (scene transitions), `scene_loading`, `script_commands`, `input_config`, `hud_render`, `debug_panels`, `music_session`, `music_guide_panel`, and `timeline_panel`.
- Full game loop: clock tick, fixed-step physics, audio sync, animation advance, script update, first-person rendering
- Scene loading with physics body creation from TOML collider/rigidbody components
- Audio source loading and spatial listener tracking
- Skeletal animation with bone matrix upload to GPU each frame
- Rhai script system with event dispatch (collisions, triggers, actions, interactions)
- Script-driven 2D HUD overlay via `DrawCommand` pipeline (replaces hardcoded HUD)
- Billboard sprite rendering for Doom-style entities
- First-person controls (WASD, mouse look, jump, sprint, interact, fire)
- Optional asset catalog integration for runtime name-based asset resolution
- Music-session lifecycle: starts a `flint-music` `ChartSession` on the shared Kira manager and hands the gamepad to `flint-input-capture` while a `music_session` component is active
- Summonable debug overlay (F3 scene panels, F4 Rendering & Effects, Music Guide and Manifest Map strips) behind the `debug-hud` feature
- Has its own `--msaa` flag; `flint play` (the CLI wrapper) does not expose it

### flint-cli

Binary crate with clap-derived command definitions. Routes commands to the appropriate subsystem crate. Scene commands: `init`, `entity`, `scene`, `query`, `schema`, `edit`, `play`, `validate`, `asset`, `render`, `gen`, `prefab`. Rhythm commands: `validate-suite`, `play-suite`, `calibrate`, `play-chart`, `replay-chart`, `render-suite`, `spike-rumble`. The pre-`edit` tools (`serve`, `preview`, `gen-preview`, `tex-edit`, `terrain-edit`, `spline-edit`) survive as hidden subcommands.

### tools/arch-analyzer

A workspace member outside `crates/`: a `syn`-based static analyzer (`flint-arch-analyzer`) that walks every crate's `Cargo.toml` and source, and writes crate, dependency-edge and per-crate metrics JSON to `tools/arch-viewer/arch-data.json`. `tools/arch-viewer` is a static HTML/JS page that renders that JSON as an interactive dependency graph. Neither is a default workspace member, and neither is a runtime dependency of the engine.

## Further Reading

- [Crate Dependency Graph](crate-graph.md) --- visual dependency diagram
- [Design Principles](../philosophy/design-principles.md) --- the principles behind these decisions
