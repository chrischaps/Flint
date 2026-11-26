# Architecture Overview

Flint is structured as a 37-member Cargo workspace (36 crates, 25 under `crates/` and the rest a package's under `packages/<name>/crates/`, plus the `tools/arch-analyzer` binary) with clear dependency layering. Each crate has a focused responsibility, and dependencies flow in one direction --- from the binaries down to core types.

## Workspace Structure

```
flint/
├── crates/
│   ├── flint-cli/          # CLI binary (clap). Entry point for all commands.
│   ├── flint-player/       # Standalone player binary with game loop, physics, audio, animation, scripting
│   ├── flint-package/      # The Package API: the Subsystem trait, its contexts, the run's typed Resources
│   ├── flint-android/      # Android entry point (NativeActivity, APK asset extraction)
│   ├── flint-script/       # Rhai scripting: ScriptEngine, ScriptSync, hot-reload
│   ├── flint-viewer/       # egui-based GUI inspector with hot-reload
│   ├── flint-debug-ui/     # DebugPanel trait + the F3/F4 panel roster (Rendering & Effects, ocean, sky, ...)
│   ├── flint-particles/    # GPU-instanced particle system with pooling and emission shapes
│   ├── flint-animation/    # Two-tier animation: property tweens + skeletal/glTF
│   ├── flint-audio/        # Kira spatial audio: 3D sounds, ambient loops, triggers
│   ├── flint-audio-worklet/# Kira mixer for the browser's AudioWorklet (its own small wasm)
│   ├── flint-terrain/      # Heightmap terrain with splat-map blending
│   ├── flint-runtime/      # Game loop infrastructure (GameClock, InputState, EventBus, GameStateMachine, pads)
│   ├── flint-physics/      # Rapier 3D integration (PhysicsWorld, CharacterController)
│   ├── flint-render/       # wgpu PBR renderer with Cook-Torrance shading + skinned mesh pipeline
│   ├── flint-import/       # File importers (glTF/GLB with skeleton/skin extraction)
│   ├── flint-pixels/       # Texel loops on load (RGBA widening, mip chains), optimized in debug builds
│   ├── flint-asset/        # Content-addressed asset storage and catalog
│   ├── flint-constraint/   # Constraint definitions and validation engine
│   ├── flint-query/        # PEG query language (pest parser)
│   ├── flint-scene/        # TOML scene serialization/deserialization
│   ├── flint-ecs/          # hecs wrapper with stable IDs, names, hierarchy
│   ├── flint-schema/       # Component/archetype schema loading and validation
│   ├── flint-geometry/     # Ocean waves, splines, ray meshes
│   └── flint-core/         # Fundamental types: EntityId, Transform, Vec3, etc.
├── packages/               # One folder a package: its package.toml, and its schemas/ and crates/
│   ├── ai/crates/
│   │   ├── flint-asset-gen/    # AI asset generation: providers, style guides, batch resolution
│   │   ├── flint-procgen-ai/   # AI-assisted procgen: ProcGenAgent trait, spec creation/refinement
│   │   └── flint-ai-core/      # What every paid AI call goes through: transport, retry, prices, ledger, cost meter
│   ├── music/crates/
│   │   └── flint-music/    # Music: suite manifests, manifest-named buses, tempo maps, conductor, offline render
│   ├── procgen/crates/
│   │   ├── flint-procgen/      # Procedural generation: Generator trait, registry, tree/texture/creature generators; the player's asset maker
│   │   └── flint-procgen-cli/  # `flint gen` for a spec, the spec editors, check's spec passes, as points the kernel asks
│   ├── rhythm/crates/
│   │   ├── flint-rhythm/       # The rhythm layer over music: charts, judgment, coherence, ladder, seam, replay, haptics
│   │   ├── flint-input-capture/# 1 kHz gamepad capture thread stamped against the audio clock
│   │   └── flint-rhythm-game/  # The package's parts: the chart judge's subsystem, `flint music calibrate|chart`
│   ├── living/crates/
│   │   ├── flint-living/       # Living props at run time, and the package's player, F3 panel and CLI parts
│   │   └── flint-wooze/        # The wooze field
│   └── <name>/             # debug-hud, engine: manifests and schemas, for now
├── tools/
│   ├── arch-analyzer/      # syn-based workspace analyzer that emits crate/dependency/metrics JSON
│   └── arch-viewer/        # Static web page that renders that JSON as an interactive graph
├── schemas/                # Default component, archetype, and constraint definitions
├── tests/fixtures/         # Frozen scenes the goldens and CLI tests load
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

Entity IDs are 64-bit integers that each world hands out from its own counter and never recycles. A scene spawns its entities in name order, so the same scene gets the same ids in every run, and a host that replaces its world (a transition, a reload) starts the next world past the last one's ids. Each world keys its own maps by `EntityId`: there is no second handle to translate to.

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
| ECS | flint-ecs (its own maps) | Components are TOML values, so a typed ECS stored nothing (#691) |
| Rendering | wgpu 30 | Cross-platform, modern GPU API |
| Windowing | winit 0.30 | `ApplicationHandler` trait pattern |
| Physics | Rapier 3D 0.22 | Mature Rust physics, character controller |
| Audio | Kira 0.11 | Rust-native, game-focused, spatial audio |
| GUI | egui 0.36 | Immediate-mode, easy integration with wgpu; also drives the debug panels and player HUD |
| Gamepads | gilrs + gilrs-core | Frame-rate polling and a script's `rumble()` through `flint-runtime`'s `pad`; direct 1 kHz polling and clock-stamped rumble in `flint-input-capture` |
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
      ├──► flint-render    (renderer)    ├──► flint-rhythm-game (rhythm sessions: flint-rhythm,
      ├──► flint-constraint(validation)  │      flint-input-capture's 1 kHz pad, over flint-music)
      ├──► flint-asset     (catalog)     ├──► flint-animation (tweens + skeletal)
      ├──► flint-asset-gen (AI gen)      ├──► flint-particles (GPU particles)
      ├──► flint-procgen   (proc gen)    ├──► flint-script    (Rhai scripting)
      ├──► flint-rhythm-game (chart tools) ├──► flint-terrain (heightmap terrain)
      │                                  ├──► flint-debug-ui  (F3/F4 panels, optional)
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

`flint-cli` depends on `flint-player` directly: `flint play` is the player embedded in the CLI, and the music and rhythm packages' commands (`flint music render`, `chart play`, `chart replay`, ...) are their crates' own (`flint-music`, `flint-rhythm-game`), listed from the package manifests, and run without a scene at all. `flint-procgen-ai` is tool-time only: `flint-cli` links it, the player does not. Every crate but `flint-android` is a default workspace member, so plain `cargo test` runs all their tests.

## Crate Details

### flint-core

Fundamental types shared by all crates. Minimal external dependencies (`thiserror`, `serde`, `sha2`).

- `EntityId` --- stable 64-bit entity identifier
- `ContentHash` --- SHA-256 based content addressing
- `Transform`, `Vec3`, `Color` --- geometric primitives
- `FlintError` --- base error type

What only some features read lives a layer up, so an edit there doesn't rebuild every crate (#754): the wooze field is `flint-wooze`, and the ocean's wave spectrum, spline sampling and ray meshes are `flint-geometry`.

### flint-wooze

The wooze field (#213): how woozy any place in a scene is, one function of position and time that living props, shallow wooze, woozy grass and the shading boil all read. Also the `shallow_wooze` component's name. Depends on `flint-core` and `flint-ecs`. The living props read it, and so does procgen's `living` part; the player, the viewer and the CLI reach it only through flint-living's parts (#1558): a build without the living package links none of it. The renderer takes the field's answers as frame data and links none of it either (#1330). Without the field, procgen still reads and settles a scene's `[wooze]` table, and `flint check` holds it to its schema alone (#1516).

### flint-geometry

- `ocean` --- the Gerstner wave spectrum, the one source of the ocean surface for the renderer and for scripts' buoyancy
- `spline` --- Catmull-Rom path sampling with twist
- `ray_mesh` --- rays against the triangles a game draws (ADR 0105)
- `components` --- the `ocean`, `ocean_contact`, `water_body` and `spline` component names

Depends only on `flint-core`.

### flint-schema

Loads component and archetype definitions from TOML files. Provides a registry for introspection. Supports field types (`bool`, `i32`, `i64`, `f32`, `f64`, `string`, `vec2`, `vec3`, `vec4`, `color`, `transform`, `enum`, `entity_ref`) with validation constraints. Since the scene loader started applying field defaults and validating on load, the registry is consulted at runtime as well as by `flint validate`.

### flint-ecs

The world, in maps of its own (#691; it once wrapped a hecs world that stored nothing):
- One record per `EntityId`: the entity's name and its dynamic components
- Named entity lookup both ways, each a map access
- Parent-child links both ways, children in name order; despawning an entity takes its subtree
- A component index (component name to the entities that have it) that every write keeps: `get_components_mut` hands out a `ComponentsMut` guard, not the map
- A per-world ID counter, and ordered walks (`entities_with_component` by id, `all_entities` by name), so a scene loads the same way every run

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

### flint-pixels

The loops that touch every texel of a texture on its way to the GPU: widening a decoded image to RGBA8 and box-filtering its mip chain (colour averaged in linear light). They live outside `flint-render` so the workspace's dev profile can build them at `opt-level = 3` with the image decoders, glTF and SHA-256, while the renderer stays unoptimized and quick to rebuild. A debug `flint render` of a scene with a 32 MB model went from 19 s to 2.6 s, its pixels unchanged (#326). No dependencies.

### flint-render

wgpu 30 PBR renderer with:
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
- `DebugPanel` trait (`name`, `ui`, `sync(&mut PanelCtx)`, `weight`, `is_open`, `toggle`, `layout`, dirty tracking) and `PanelLayout::{SideRight, Bottom}`; the player syncs every panel in one loop
- `assign_columns()` balances open panels across three side columns by each panel's `weight()`
- Panel roster: Rendering & Effects (`RENDER_DEBUG_PANEL`, F4), Ocean, Camera, Grass, Particles, Styles, and `ComponentPanel`, drawn from any component schema whose fields say `panel = ...` (#744)
- `ScenePanels` --- the registration point a package adds its scene's panel to (the living package's Living Props panel, #1558), and `DebugPanel::paint_world` for what a panel paints in the world
- Depends on `flint-render` (it mirrors and writes through to renderer state), `flint-ecs`, `flint-particles`, `flint-schema`, `flint-terrain`, and `flint-scene`
- Optional in the player behind the default-on `debug-hud` cargo feature; feature-off builds carry no debug surface

### flint-runtime

Game loop infrastructure for interactive scenes:
- `GameClock` --- fixed-timestep accumulator (1/60s default)
- `InputState` and `InputConfig` --- keyboard/mouse/gamepad tracking with TOML-configured action bindings
- `EventBus` --- the frame's game events; the player owns it, physics pushes its contacts into it, and extensions, style changes and scripts push their own
- `RuntimeSystem` trait --- `initialize` and `update`, shared by scripts, audio, animation and particles; the player calls each by type (there is no registry)
- `GameStateMachine` --- a stack of game states (play, pause, menu), each a `StateConfig` of `SystemPolicy` (`Run` or `Pause`) by system name; `register_state` refuses a name the host has no system for
- `PersistentStore` --- key-value data that survives scene transitions
- `pad` --- the gamepad backend every pad reader goes through: gilrs with the `gamepad` feature (off by default here; the player's turns it on), else a stand-in that never finds a backend
- `PadRumble` --- a script's `rumble(pad, strength, seconds)`: both motors through gilrs's force-feedback server, in 50 ms ticks

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

### flint-audio-worklet

The web player's mixer, run in the browser's audio thread (#136). Mixing on the main thread made every long frame a late buffer on a phone, so this crate is a second, small wasm that runs Kira inside an `AudioWorkletProcessor`. It knows nothing of Flint: the page posts it sounds as samples and one batch of `protocol::Command`s a frame, and the processor asks it for 128 frames at a time. `Mixer` is plain Rust, tested natively; the C ABI the processor's JavaScript calls is wasm-only. `trunk` builds it with `cargo build -p flint-audio-worklet --target wasm32-unknown-unknown --profile worklet` before each web build.

### flint-music

Music for any game, with no game rules in it:
- `SuiteManifest` (tempo map, sections, as many stem buses as the suite has, named by the manifest and mixed in the order it declares them), with `validate_manifest` / `validate_manifest_assets` checking it and its stems on disk
- `Conductor`, `TempoMap`, `MusicalPosition` --- beat/bar arithmetic and the `ClockBridge` that maps the audio clock onto game time
- `SuiteSession`, `BusMixer`, `Scheduler` / `EventScript` --- stems sample-locked on one clock tick, per-bus gain, low-pass and detune, bus events on the musical grid
- `latency` --- the newest output-latency measurement and tap calibration on record
- `render_offline` / `write_wav` --- headless suite rendering behind `flint music render`
- Depends only on `flint-core` and `flint-audio` plus kira, symphonia, hound, toml_edit
- The `music` package's own crate, in its folder (`packages/music/crates/flint-music`, #1555): its `player` feature is the scene's music subsystem (`flint_music::player`, a `flint_package::Package` with a chart-judge registration point the rhythm package fills), its `cli` feature the `flint music validate|play|render` commands (`flint_music::cli`). The player's and the CLI's build scripts list both from the package folder, so neither names the crate

### flint-rhythm

The rhythm-game layer over `flint-music` ("linear composition, adaptive playback"). Originated in Starchild, engine-generic:
- `Chart` (continuous input curves, discrete pulses, scene cues) and the rhythm contract a judged suite keeps (the six buses, `[reintegration]`, a judgment window per section): `validate_manifest` runs flint-music's checks and adds its own, `validate_chart` cross-checks a chart against its suite
- `ChartSession` / `ChartCore` --- the per-frame judgment loop producing `ConductedFrame` values that scripts read through the `conducted_*` API
- `Coherence`, `Judge`, `Ladder` / `LadderDriver`, `Reintegrator` --- the disintegration ladder, hysteresis, and the seam that brings the ensemble back in
- `GradientDriver`, `HapticsDriver` --- error-driven audio gradient, rumble entrainment
- `SessionWriter` / `read_session` / `synthesize` --- `.session.jsonl` recording, replay, and synthetic input profiles
- Depends on `flint-music` and `flint-core`
- The `rhythm` package's, in its folder (`packages/rhythm/crates/flint-rhythm`, #1556), with flint-input-capture and flint-rhythm-game

### flint-input-capture

A dedicated thread that owns `Gilrs` and polls it at 1 kHz (`CaptureConfig::poll_hz`), stamping each event with a latency-compensated suite sample from the `ClockBridge` and emitting `flint-rhythm` `InputEvent`s over a channel:
- `VerbMap::{Prototype, Full}` maps sticks, buttons and triggers onto the chart verb space (`lean`, `sway`, `pulse`, `press`, `flick`, `pressure_l/r`); charts never see buttons
- `measure_granularity` reports the driver's real poll cadence; `rumble` drives haptics directly over XInput
- Uses the gilrs XInput backend on Windows because the default WGI backend delivers nothing to console apps
- Reads pads through `flint-runtime`'s `pad`, as the player does; nothing outside the rhythm layer needs this crate
- Depends on `flint-core`, `flint-music`, `flint-rhythm` and `flint-runtime`
- The `rhythm` package's (`packages/rhythm/crates/flint-input-capture`, #1556)

### flint-rhythm-game

The `rhythm` package's two parts on the Package API (`packages/rhythm/crates/flint-rhythm-game`, #1556), its own crate because the session needs both flint-rhythm and flint-input-capture, and flint-input-capture depends on flint-rhythm:
- `player` feature: `player::PACKAGE`, a `flint_package::Package` that fills the music package's chart judge with `RhythmSubsystem`. The subsystem starts a `ChartSession` on the shared Kira manager, takes the pads (`Pads::take`) for the 1 kHz capture, drains it at `Stage::Clock`, sets the `conducted_*` snapshot and the music clock at `PreScripts`, and lays the disintegration ladder's post values at `Procgen` over the scene's `[post_process]` (`SceneCtx::post`). With `debug-hud` it adds the Music Guide and Manifest Map panels and their keys
- `cli` feature: `cli::extensions()`, `flint music calibrate` and `flint music chart play|replay` under the music package's `music`, and the rhythm contract `music validate` checks
- flint-player's `rhythm` feature turns `player` on and flint-cli's turns `cli` on. Both build scripts list the crate from the manifest's `[rust]`, so neither names it
- Depends on `flint-package`, `flint-music`, `flint-rhythm`, `flint-input-capture` and, by feature, `flint-script`, `flint-render`, `flint-debug-ui`, winit and wgpu

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
- `ScriptCommand` pattern --- every effect on the host (sound, styles, scenes, the camera's and post-processing's per-frame requests), carried out through one `ScriptHost` interface by `host::apply`, the one dispatcher the player and `flint render --scripts` share ([ADR 0116](https://github.com/chrischaps/Flint/blob/main/docs/adr/0116-declared-script-api.md), #697)
- `DrawCommand` pattern --- immediate-mode 2D draw primitives (text, rect, circle, line, sprite) rendered via egui
- `ScriptCallContext` with raw `*mut FlintWorld` pointer for world access during call batches, reached only through `try_world`/`call.world()`; `unsafe` is in `context.rs` alone
- Depends on `flint-physics` for raycast and camera direction access

### flint-asset-gen

AI asset generation pipeline:
- `GenerationProvider` trait with pluggable implementations (Flux, Meshy, ElevenLabs, Mock)
- `StyleGuide` --- TOML-defined visual vocabulary (palette, materials, geometry constraints) for prompt enrichment
- `SemanticAssetDef` --- maps intent (description, material, wear level) to generation requests
- Batch scene resolution with strategies: `AiGenerate`, `HumanTask`, `AiThenHuman`
- `validate_model()` --- reads a GLB with `flint_procgen::read_model` and runs `flint_procgen::validate_model` with the style's constraints (`StyleGuide::validation_constraints`): the checks `flint gen --validate` runs
- `BuildManifest` --- provenance tracking (provider, prompt, content hash) for all generated assets
- `FlintConfig` --- layered configuration for API keys and provider settings
- `Calls` --- what every provider request goes through: one `flint-ai-core` transport and retry policy, the run's `CostMeter` (asked before each paid call with the units it bills, recorded once the provider accepts it) and the job store. Providers are tested end to end over a `ScriptedTransport`
- `JobStore` --- a Meshy model (a preview task, then a refine of it, #1187) is a `GenerationJob` in `.flint/jobs/` from the moment Meshy accepts (and bills) the preview, naming both tasks (`remote_id`, `refine_id`) and updated as each is polled; `GenerationProvider::resume` finishes one a run didn't see through, from either stage

### flint-procgen

Procedural generation framework, the procgen package's own crate (`packages/procgen/crates/flint-procgen`, #1557):
- `Generator` trait --- pluggable generator interface with `generate()`, `param_schema()`, `estimate_cost()`
- `GeneratorRegistry` --- register and look up generators by type name
- `ProcGenSpec` --- TOML spec format with metadata, seed config, and generator parameters
- Built-in generators: `tree_v1` (L-system/space colonization trees), `texture_v1` (PBR texture maps), `creature_v1`, `form_v1` (one abstract living form on a spine)
- `ProcGenCache` --- LRU cache keyed by (spec_hash, seed) with memory budget, over a `DiskCache` tier in `.flint/procgen-cache/` that keeps outputs between runs
- Algorithmic building blocks: noise (Perlin, simplex, Worley, FBM), L-system engine, mesh builder, space colonization
- The living side of a generator: `LivingGenerator` (a structure and its poses), `LivingRig`, `LivingSkeleton`, `LivingPose`, and the `[living]` / `[wooze]` scene tables. What runs them over time is `flint-living`
- The package's part of the player, behind its `player` feature (`player/`): `PACKAGE`, which a launcher lists; the run's resolver in the resources; the `AssetMaker` the kernel calls at a scene's load, for spawned entities and each frame (catalog > procgen > file > placeholder); `GpuSink`; and `LoadSteps`, where living's props join the load

### flint-procgen-cli

The procgen package's part of the `flint` command line (`packages/procgen/crates/flint-procgen-cli`, #1557). The commands stay the kernel's, so `flint --help` is the same in every build; what the package brings it registers as points (`flint_package::cli::Points`):
- `SpecGen` --- `flint gen <spec>`: generate, validate, register, batch; it asks the ai package's steps (`AiSteps`) and living's contact sheet (`StripSheet`) by points of its own
- `SpecEditors` --- the procgen previewer and the texture node editor a `.procgen.toml` opens in (`flint edit`, `gen-preview`, `tex-edit`)
- `SpecChecks` --- what `flint check` asks of a spec: it parses, its params fit its generator, a terrain spec's issues

### flint-living

Living props at run time (#1024), split out of `flint-procgen` so that only a game with living props links the scheduler; the living package's crate (`packages/living/crates/flint-living`, #1558), whose parts are features:
- `LivingProps` --- every lineage (one skeleton and rig a spec and lineage seed) and every prop on it, each with its ring of poses a few steps ahead of its clock, its echoes and its knobs' layers
- The seam with the resolver: a host keeps `LivingProps` beside its `ProcGenResolver`, which lends it its specs (`living_spec`), its generators and what its queue left of the frame budget (`budget_left_ms`); `add_prop`, `process`, `step_now`, `process_beside` and `settle` take the resolver
- `BlendFrame` / `blend_prop` --- each frame's two-pose blend per part, written only where something moved
- Knobs (`tune_*`, `LivingScope`, `LivingLayers`) and the A/B fork (`fork`, `swap_cache`, `redraw_all`)
- `player` --- the package's part of the player: `player::PACKAGE`, the living subsystem (the wooze field and its clock, the shading boil, shallow wooze, woozy grass, the hand tuning, the A/B), the `living`, `wooze` and `debug` script modules, its step of procgen's load and its settle of `[living]` / `[wooze]`
- `debug-hud` --- the F3 Living Props panel (`panel`), with the debug-hud package listed too (the manifest's `[with.debug-hud]`)
- `cli` --- `cli::extensions()`: no command of its own; it fills `flint render`'s stage point, `flint check`'s table checks, `gen --strip` and the scene viewer's field clocks

### flint-procgen-ai

AI-assisted procedural generation (tool-time only):
- `ProcGenAgent` trait --- `interpret_spec()`, `create_spec_from_prompt()`, `refine_spec()`; object-safe (`Box<dyn ProcGenAgent>`)
- `MockAgent` --- local and deterministic (no network, no key): specs start from the generator's `param_schema()` defaults, a small keyword table shapes them ("tall", "autumn", "brick"), and `"tree"` resolves to `tree_v1`
- `schema` --- `repair_params` fixes what `flint_procgen::validate_params` (the one validator, #718: unknown and ignored params, types, ranges, enums, hex colours, required keys, at every depth) reports about a spec's params; `interpret_spec` reports those issues as suggestions with a repaired `revised_spec`
- `AnthropicAgent` --- asks Claude over the Messages API (`ureq`, API version `2026-10-21`, default model `claude-opus-5-5`). The system prompt holds the generator's `param_schema()` verbatim, the spec format and a valid example, and it is the same for every call to that generator, so it is cached. Each reply's params are checked against the schema and then actually generated (an empty mesh counts as a failure). A failed reply goes back with the validator's report for another round, up to `max_iterations`; after that the schema repair clamps what it can. `refine_spec` sends the reference image as base64, beside a picture of what the spec builds now: a texture's own pixels, or whatever the caller's `Previewer` draws (`with_previewer`; `flint-cli` renders meshes with the headless model renderer, so this crate stays free of `flint-render`). With both images it measures their distance (`measure::distance`: mean colour, luminance histogram, 16x16 layout) and loops, rendering each round's result and sending it back while the distance falls, up to `refine_rounds`; the closest spec is returned. `Report.distances` holds the distance per round and `Report.kept` the round returned. The `*_reported` methods return a `Report` with each round's issues and token usage. 429/529/5xx responses are retried (honouring `retry-after`); 401 is not.
- `AgentConfig` --- `Debug` redacts the API key and serialization skips it; `ANTHROPIC_API_KEY` takes precedence over a key in the config; optional `effort`
- `cargo run -p flint-procgen-ai --example walkthrough -- out/` writes create/refine specs for `flint gen`; `--example claude_live -- out/` does the same against the real API (spends tokens)
- Its transport, retry loop and cost meter are `flint-ai-core`'s

### flint-ai-core

What every paid AI call in the tools goes through, shared by `flint-procgen-ai`, `flint-asset-gen` and the CLI (#721; tool-time only):
- `http` --- the one-method `Transport` (GET and POST, byte bodies), `UreqTransport`, and `ScriptedTransport`, which replays canned responses and keeps every request, so a provider is tested with no network
- `retry` --- one `RetryPolicy`: 408, 429, 5xx and 529 and network failures are retried, waiting the base delay and then twice that, or the provider's `retry-after`, at most a minute
- `cost::PriceTable` --- the prices as data: `prices.toml` in the crate (Anthropic's per-token rates by model; Flux per megapixel, ElevenLabs per second, Meshy per credit), under a project's `.flint/prices.toml`. An unknown model is priced as the dearest
- `CostLedger` --- `.flint/ai_costs.jsonl` at the project root, one JSON object per paid call (the procgen agent's older `.flint/procgen_costs.json` is read beside it); `CostSummary` is the report `flint gen --cost-report` and `flint asset costs` print
- `CostMeter` --- `admit` (a token-billed reply: caps `max_tokens` or refuses) and `admit_call` (a call priced per unit) before a call, `record` / `record_call` after it; refuses with `cost ceiling reached` before any call that could pass `--max-cost`

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

### flint-package

The Package API ([ADR 0137](https://github.com/chrischaps/Flint/blob/main/docs/adr/0137-the-public-package-api.md)): what a package's crate builds against, so it depends on this small kernel crate and never on `flint-player` or `flint-cli`. It depends only on kernel crates (flint-core, flint-ecs, flint-runtime, flint-script, flint-render, flint-audio, flint-physics), never on a package's.
- `Subsystem`: a package's running part, with a hook for each moment of a run (a scene coming and going, each `Stage` of the frame, each fixed physics step, a script's command, a debug key, a click, the frame's renderer inputs and encoder, the HUD's root `egui::Ui`, the exit report), each defaulting to nothing
- The contexts each hook is lent (`SceneCtx`, `StageCtx`, `FixedCtx`, `FrameCtx`, `EncodeCtx`, `ReportCtx`): every field a kernel type
- `Subsystem::fixed_update(&mut FixedCtx)`: once for each fixed physics step that steps (two in a frame at 30 Hz, none on most frames at 144), after the character controller and the physics sync-in and before Rapier's step, so a force it applies is integrated by that step. `FixedCtx` lends the world, the physics (`PhysicsSystem::body_of`, `bodies_mut`, `colliders`, `queries`, and `rapier_mut` for all three at once, as Rapier's ray-cast vehicle controller takes them), the step's `dt`, its index in the frame, held input, the event bus, the game state's policy and the resources; no scripts, renderer, camera or sound. A paused state and a capture (`flint render`, which runs no physics) skip it. `flint_package::physics::rapier` is the engine's Rapier
- `Resources`: the run's typed values, one per Rust type, through which one package lends state to another without the kernel naming either (procgen's resolver to living's props; the debug HUD's `DebugPanels` to any subsystem with a panel). A missing one means the package isn't in the build
- `Stage` (re-exported as `flint_player::Stage`), `Note` (what a debug key says on screen), `Pads`, `PlayerInput` and `Button` (a click without winit), and `egui`, re-exported so a package builds against the engine's

The player keeps the ordered list of subsystems and lends each context from its own fields. Registration comes next (#1552).

### flint-player

Standalone player binary that wires together runtime, physics, audio, animation, particles, scripting, music sessions, and rendering. The `player_app` module is decomposed into named lifecycle files ([ADR 0062](https://github.com/chrischaps/Flint/blob/main/docs/adr/0062-player-app-split-into-lifecycle-files.md)): `init` (construction and loading), `frame` (the per-frame loop), `events` (window and key handling), `transition` (scene transitions), `scene_loading`, `script_commands`, `input_config`, `hud_render`, `debug_panels`, `music_session`, `music_guide_panel`, and `timeline_panel`.
- Full game loop: clock tick, fixed-step physics, audio sync, animation advance, script update, first-person rendering
- Scene loading with physics body creation from TOML collider/rigidbody components
- Audio source loading and spatial listener tracking
- Skeletal animation with bone matrix upload to GPU each frame
- Rhai script system with event dispatch (collisions, triggers, actions, interactions)
- Script-driven 2D HUD overlay via `DrawCommand` pipeline (replaces hardcoded HUD)
- Billboard sprite rendering for Doom-style entities
- First-person controls (WASD, mouse look, jump, sprint, interact, fire)
- Optional asset catalog integration for runtime name-based asset resolution
- Package subsystems by registration: the music package's session and the rhythm package's chart judge (`flint-rhythm-game`) are listed from the manifests, so the player names neither; the player lends its pads (`Pads`) to whichever takes them
- Summonable debug overlay (F3 scene panels, F4 Rendering & Effects, Music Guide and Manifest Map strips) behind the `debug-hud` feature
- One launch (`player_args.rs`, #705): `flint_player::PlayerArgs` is every flag, `flint_player::launch` turns them into a running `PlayerApp`, and both `flint-player` (which adds `--bundle`) and `flint play` call it, so the two take the same flags and play a scene the same
- One set of options (`play_options.rs`, #1142): the desktop's flags (`PlayerArgs::options`), the web page's query string (`PlayOptions::from_query`) and Android's `dev_profile.toml` each build a `flint_player::PlayOptions`, and `PlayOptions::apply` sets the `PlayerApp` from it, so a value means the same whichever host read it. A bad value stops the desktop before its window opens. The page and Android warn of it and play on with the default

### flint-cli

Binary crate with clap-derived command definitions. Routes commands to the appropriate subsystem crate. Commands: `init`, `entity`, `scene`, `query`, `schema`, `script`, `edit`, `validate`, `check`, `frame`, `profile`, `examples`, `env`, `bundle`, `export`, `music`, `goldens`, `prefab`, `asset`, `package`, `build`, `play`, `playtest`, `capture`, `gen`, `render`, `dxc` and `style`. `music` holds the rhythm-game tools (`validate`, `play`, `render`, `calibrate`, `chart play`, `chart replay`), top-level commands until #709; their old names ran for one release and are gone since #1141. The pre-`edit` tools (`serve`, `preview`, `gen-preview`, `tex-edit`, `terrain-edit`, `spline-edit`) survive as hidden subcommands. [The CLI reference](../cli-reference/overview.md) covers each one.

### tools/arch-analyzer

A workspace member outside `crates/`: a `syn`-based static analyzer (`flint-arch-analyzer`) that walks every crate's `Cargo.toml` and source, and writes crate, dependency-edge and per-crate metrics JSON to `tools/arch-viewer/arch-data.json`. `tools/arch-viewer` is a static HTML/JS page that renders that JSON as an interactive dependency graph. Neither is a default workspace member, and neither is a runtime dependency of the engine.

## Further Reading

- [Crate Dependency Graph](crate-graph.md) --- visual dependency diagram
- [Design Principles](../philosophy/design-principles.md) --- the principles behind these decisions
