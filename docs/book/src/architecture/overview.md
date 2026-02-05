# Architecture Overview

Flint is structured as a fourteen-crate Cargo workspace with clear dependency layering. Each crate has a focused responsibility, and dependencies flow in one direction --- from the binaries down to core types.

## Workspace Structure

```
flint/
├── crates/
│   ├── flint-cli/          # CLI binary (clap). Entry point for all commands.
│   ├── flint-player/       # Standalone player binary with game loop and physics
│   ├── flint-viewer/       # egui-based GUI inspector with hot-reload
│   ├── flint-runtime/      # Game loop infrastructure (GameClock, InputState, EventBus)
│   ├── flint-physics/      # Rapier 3D integration (PhysicsWorld, CharacterController)
│   ├── flint-render/       # wgpu PBR renderer with Cook-Torrance shading
│   ├── flint-import/       # File importers (glTF/GLB)
│   ├── flint-asset/        # Content-addressed asset storage and catalog
│   ├── flint-constraint/   # Constraint definitions and validation engine
│   ├── flint-query/        # PEG query language (pest parser)
│   ├── flint-scene/        # TOML scene serialization/deserialization
│   ├── flint-ecs/          # hecs wrapper with stable IDs, names, hierarchy
│   ├── flint-schema/       # Component/archetype schema loading and validation
│   └── flint-core/         # Fundamental types: EntityId, Transform, Vec3, etc.
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
| GUI | egui 0.30 | Immediate-mode, easy integration with wgpu |
| Scene format | TOML | Human-readable, diffable, good Rust support |
| Query parser | pest | PEG grammar, good error messages |
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
      ├──► flint-viewer (GUI)            ├──► flint-runtime  (game loop, input)
      ├──► flint-query  (queries)        ├──► flint-physics  (Rapier 3D)
      ├──► flint-scene  (load/save)      └──► flint-render   (PBR renderer)
      ├──► flint-render (renderer)               │
      ├──► flint-constraint (validation)         ▼
      ├──► flint-asset  (asset catalog)      flint-import   (glTF meshes)
      └──► flint-import (glTF import)            │
              │                                  ▼
              ▼                              flint-ecs
          flint-ecs                          flint-schema
          flint-schema                       flint-core
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
- `PhysicsWorld` --- manages Rapier rigid body and collider sets
- `PhysicsSync` --- bridges TOML component data to Rapier bodies
- `CharacterController` --- kinematic first-person movement with gravity, jumping, and ground detection
- Uses kinematic bodies for player control, static bodies for world geometry

### flint-player

Standalone player binary that wires together runtime, physics, and rendering:
- Full game loop: clock tick, fixed-step physics, character controller, first-person rendering
- Scene loading with physics body creation from TOML collider/rigidbody components
- First-person controls (WASD, mouse look, jump, sprint)

### flint-cli

Binary crate with clap-derived command definitions. Routes commands to the appropriate subsystem crate. Commands: `init`, `entity`, `scene`, `query`, `schema`, `serve`, `play`, `validate`, `asset`, `render`.

## Further Reading

- [Crate Dependency Graph](crate-graph.md) --- visual dependency diagram
- [Design Principles](../philosophy/design-principles.md) --- the principles behind these decisions
