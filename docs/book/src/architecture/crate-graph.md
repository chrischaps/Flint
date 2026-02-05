# Crate Dependency Graph

This page shows how Flint's crates depend on each other. Dependencies flow downward --- higher crates depend on lower ones, never the reverse.

## Dependency Diagram

```
                        ┌─────────────┐
                        │  flint-cli  │
                        │  (binary)   │
                        └──────┬──────┘
                               │
          ┌────────┬───────┬───┴───┬────────┬────────┬────────┐
          │        │       │       │        │        │        │
          ▼        ▼       ▼       ▼        ▼        ▼        ▼
     ┌─────────┐ ┌─────┐ ┌─────┐ ┌──────┐ ┌──────┐ ┌──────┐ ┌──────┐
     │  query  │ │scene│ │serve│ │render│ │const-│ │asset │ │import│
     │         │ │     │ │     │ │      │ │raint │ │      │ │      │
     └────┬────┘ └──┬──┘ └──┬──┘ └──┬───┘ └──┬───┘ └──┬───┘ └──┬───┘
          │         │       │       │        │        │        │
          │         │       │       │     ┌──┘        │        │
          │         │       │       │     │           │        │
          ▼         ▼       ▼       ▼     ▼           ▼        ▼
     ┌──────────────────────────────────────────┐  ┌──────────────┐
     │               flint-ecs                  │  │              │
     │  (hecs wrapper, stable IDs, hierarchy)   │  │  flint-asset │
     └─────────────────┬────────────────────────┘  └──────┬───────┘
                       │                                  │
                       ▼                                  │
               ┌──────────────┐                           │
               │ flint-schema │                           │
               │              │                           │
               └──────┬───────┘                           │
                      │                                   │
                      ▼                                   ▼
               ┌─────────────────────────────────────────────┐
               │              flint-core                     │
               │  (EntityId, Vec3, Transform, ContentHash)   │
               └─────────────────────────────────────────────┘
```

## Dependency Details

| Crate | Depends On | Depended On By |
|-------|-----------|----------------|
| `flint-core` | *(none)* | all other crates |
| `flint-schema` | core | ecs, constraint |
| `flint-ecs` | core, schema | scene, query, render, constraint, cli |
| `flint-query` | core, ecs | constraint, cli |
| `flint-scene` | core, ecs, schema | cli |
| `flint-constraint` | core, ecs, schema, query | cli |
| `flint-asset` | core | import, cli |
| `flint-import` | core, asset | cli |
| `flint-render` | core, ecs, schema | cli |
| `flint-cli` | all crates | *(binary entry point)* |

## Key Properties

**Acyclic.** The dependency graph has no cycles. This is enforced by Cargo and ensures clean compilation ordering.

**Layered.** Crates form clear layers:
1. **Core** --- fundamental types (`flint-core`)
2. **Schema** --- data definitions (`flint-schema`)
3. **Storage** --- entity and asset management (`flint-ecs`, `flint-asset`)
4. **Logic** --- query, scene, constraint, import, render
5. **Interface** --- CLI binary (`flint-cli`)

**Independent subsystems.** The constraint system, asset system, and render system don't depend on each other. They all flow through the CLI. This means you can build and test each subsystem in isolation.

## External Dependencies

Key third-party crates used across the workspace:

| Crate | Used By | Purpose |
|-------|---------|---------|
| `hecs` | flint-ecs | Underlying ECS implementation |
| `toml` | flint-schema, flint-scene, flint-constraint, flint-asset | TOML parsing and serialization |
| `serde` | all crates | Serialization framework |
| `pest` | flint-query | PEG parser generator |
| `wgpu` | flint-render | GPU abstraction layer |
| `winit` | flint-render | Window management |
| `clap` | flint-cli | Command-line argument parsing |
| `thiserror` | all crates | Error derive macros |
| `sha2` | flint-core, flint-asset | SHA-256 hashing |
| `gltf` | flint-import | glTF file parsing |
