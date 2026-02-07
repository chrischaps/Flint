# Architecture Overview

Flint is structured as an eighteen-crate Cargo workspace with clear dependency layering. Each crate has a focused responsibility, and dependencies flow in one direction --- from the binaries down to core types.

## Workspace Structure

```
flint/
├── crates/
│   ├── flint-cli/          # CLI binary (clap). Entry point for all commands.
│   ├── flint-asset-gen/    # AI asset generation: providers, style guides, batch resolution
│   ├── flint-player/       # Standalone player binary with game loop, physics, audio, animation, scripting
│   ├── flint-script/       # Rhai scripting: ScriptEngine, ScriptSync, hot-reload
│   ├── flint-viewer/       # egui-based GUI inspector with hot-reload
│   ├── flint-animation/    # Two-tier animation: property tweens + skeletal/glTF
│   ├── flint-audio/        # Kira spatial audio: 3D sounds, ambient loops, triggers
│   ├── flint-runtime/      # Game loop infrastructure (GameClock, InputState, EventBus)
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
├── schemas/                # Default component, archetype, and constraint definitions
├── games/                  # Game projects with their own schemas/scripts/scenes/assets
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

The TOML file on disk is canonical. In-memory state is derived from it. The `serve --watch` viewer re-parses the entire file on change rather than attempting incremental updates. This is simpler and avoids synchronization bugs.

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
| GUI | egui 0.30 | Immediate-mode, easy integration with wgpu |
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
  (scene authoring)                (interactive gameplay)
      │                                  │
      ├──► flint-viewer    (GUI)         ├──► flint-runtime   (game loop, input)
      ├──► flint-query     (queries)     ├──► flint-physics   (Rapier 3D)
      ├──► flint-scene     (load/save)   ├──► flint-audio     (Kira spatial audio)
      ├──► flint-render    (renderer)    ├──► flint-animation (tweens + skeletal)
      ├──► flint-constraint(validation)  ├──► flint-script    (Rhai scripting)
      ├──► flint-asset     (catalog)     └──► flint-render    (PBR + skinned mesh)
      ├──► flint-asset-gen (AI gen)              │
      └──► flint-import    (glTF import)         ▼
              │                              flint-import  (glTF meshes + skins)
              ▼                                  │
          flint-ecs                              ▼
          flint-schema                       flint-ecs
          flint-core                         flint-schema
                                             flint-core
```

## Crate Details

### flint-core

Fundamental types shared by all crates. Minimal external dependencies (`thiserror`, `serde`, `sha2`).

- `EntityId` --- stable 64-bit entity identifier
- `ContentHash` --- SHA-256 based content addressing
- `Transform`, `Vec3`, `Color` --- geometric primitives
- `FlintError` --- base error type

### flint-schema

Loads component and archetype definitions from TOML files. Provides a registry for introspection. Supports field types (`bool`, `i32`, `f32`, `string`, `vec3`, `enum`, `entity_ref`) with validation constraints.

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
- Entity tree with selection
- Component property editor
- Constraint violation overlay
- Hot-reload via file watching (`serve --watch`)

### flint-runtime

Game loop infrastructure for interactive scenes:
- `GameClock` --- fixed-timestep accumulator (1/60s default)
- `InputState` --- keyboard and mouse tracking with action bindings
- `EventBus` --- decoupled event dispatch between systems
- `RuntimeSystem` trait --- standard interface for update/render systems

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

### flint-animation

Two-tier animation system:
- **Tier 1: Property tweens** --- `AnimationClip` with keyframe tracks targeting transform properties (position, rotation, scale) or custom fields. Step, Linear, and CubicSpline interpolation. Clips defined in `.anim.toml` files.
- **Tier 2: Skeletal animation** --- `Skeleton` and `SkeletalClip` types for glTF skin/joint hierarchies. GPU vertex skinning via bone matrix storage buffer. Crossfade blending between clips.
- `AnimationSync` bridges ECS `animator` components to property playback
- `SkeletalSync` bridges ECS to skeletal playback with bone matrix computation

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

### flint-player

Standalone player binary that wires together runtime, physics, audio, animation, scripting, and rendering:
- Full game loop: clock tick, fixed-step physics, audio sync, animation advance, script update, first-person rendering
- Scene loading with physics body creation from TOML collider/rigidbody components
- Audio source loading and spatial listener tracking
- Skeletal animation with bone matrix upload to GPU each frame
- Rhai script system with event dispatch (collisions, triggers, actions, interactions)
- Script-driven 2D HUD overlay via `DrawCommand` pipeline (replaces hardcoded HUD)
- Billboard sprite rendering for Doom-style entities
- First-person controls (WASD, mouse look, jump, sprint, interact, fire)
- Optional asset catalog integration for runtime name-based asset resolution

### flint-cli

Binary crate with clap-derived command definitions. Routes commands to the appropriate subsystem crate. Commands: `init`, `entity`, `scene`, `query`, `schema`, `serve`, `play`, `validate`, `asset`, `render`.

## Further Reading

- [Crate Dependency Graph](crate-graph.md) --- visual dependency diagram
- [Design Principles](../philosophy/design-principles.md) --- the principles behind these decisions
