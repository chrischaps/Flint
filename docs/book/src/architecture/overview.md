# Architecture Overview

Flint is structured as a seven-crate Cargo workspace with clear dependency layering. Each crate has a focused responsibility, and dependencies flow in one direction --- from the CLI down to core types.

## Workspace Structure

```
flint/
├── crates/
│   ├── flint-cli/          # CLI binary (clap). Entry point for all commands.
│   ├── flint-query/        # PEG query language (pest parser)
│   ├── flint-scene/        # TOML scene serialization/deserialization
│   ├── flint-render/       # wgpu-based 3D renderer
│   ├── flint-constraint/   # Constraint definitions and validation engine
│   ├── flint-asset/        # Content-addressed asset storage and catalog
│   ├── flint-import/       # File importers (glTF/GLB)
│   ├── flint-ecs/          # hecs wrapper with stable IDs, names, hierarchy
│   ├── flint-schema/       # Component/archetype schema loading and validation
│   └── flint-core/         # Fundamental types: EntityId, Transform, Vec3, etc.
├── schemas/                # Default component, archetype, and constraint definitions
├── demo/                   # Showcase scene and build scripts
└── docs/                   # This documentation
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

### Error Handling

All crates use `thiserror` for error types. Each crate defines its own error enum and a `Result<T>` type alias. Errors propagate upward through the crate hierarchy.

## Technology Choices

| Component | Technology | Rationale |
|-----------|------------|-----------|
| Language | Rust | Performance, safety, game ecosystem |
| ECS | hecs | Lightweight, standalone, well-tested |
| Rendering | wgpu 23 | Cross-platform, modern GPU API |
| Windowing | winit 0.30 | `ApplicationHandler` trait pattern |
| Scene format | TOML | Human-readable, diffable, good Rust support |
| Query parser | pest | PEG grammar, good error messages |
| CLI framework | clap (derive) | Ergonomic, well-documented |
| Error handling | thiserror + anyhow | Typed errors in libraries, flexible in binary |

## Data Flow

Commands enter through the CLI and flow downward through the crate hierarchy:

```
User / AI Agent
      │
      ▼
  flint-cli          Parse command, dispatch to subsystem
      │
      ├──► flint-query        Parse and execute queries
      ├──► flint-scene        Load/save scene TOML
      ├──► flint-render       Render scene (viewer or headless)
      ├──► flint-constraint   Validate scene against rules
      ├──► flint-asset        Manage content-addressed assets
      └──► flint-import       Import external files (glTF)
              │
              ▼
          flint-ecs           Entity storage, ID mapping, hierarchy
              │
              ▼
          flint-schema        Component/archetype definitions
              │
              ▼
          flint-core          EntityId, Transform, Vec3, Color, errors
```

## Crate Details

### flint-core

Fundamental types shared by all crates. No external dependencies beyond `thiserror` and `serde`.

- `EntityId` --- stable 64-bit entity identifier
- `ContentHash` --- SHA-256 based content addressing
- `Transform`, `Vec3`, `Color` --- geometric primitives
- `FlintError` --- base error type

### flint-schema

Loads component and archetype definitions from TOML files. Provides a registry for introspection.

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

File importers for bringing external assets into the content-addressed store. Currently supports glTF/GLB with mesh, material, and texture extraction.

### flint-render

wgpu-based renderer. Two modes:
- **Viewer mode** --- interactive window with orbit camera, hot-reload via `serve --watch`
- **Headless mode** --- render to PNG for CI and automated screenshots

Currently renders entities as archetype-colored boxes (rooms as blue wireframes, doors as orange, furniture as green, characters as yellow).

### flint-cli

Binary crate with clap-derived command definitions. Routes commands to the appropriate subsystem crate. Commands: `init`, `entity`, `scene`, `query`, `schema`, `serve`, `validate`, `asset`, `render`.

## Further Reading

- [Crate Dependency Graph](crate-graph.md) --- visual dependency diagram
- [Design Principles](../philosophy/design-principles.md) --- the principles behind these decisions
