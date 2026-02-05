# Crate Dependency Graph

This page shows how Flint's fourteen crates depend on each other. Dependencies flow downward --- higher crates depend on lower ones, never the reverse.

## Dependency Diagram

```
          ┌─────────────┐                ┌──────────────┐
          │  flint-cli  │                │ flint-player │
          │  (binary)   │                │   (binary)   │
          └──────┬──────┘                └──────┬───────┘
                 │                              │
    ┌────┬───┬──┴──┬────┬────┬────┐    ┌───┬───┴───┬───┐
    │    │   │     │    │    │    │    │   │       │   │
    ▼    │   ▼     ▼    ▼    ▼    ▼    ▼   ▼       ▼   │
 ┌──────┐│┌─────┐┌────┐│ ┌─────┐│ ┌────────┐┌────────┐│
 │viewer│││scene││qry ││ │const││ │runtime ││physics ││
 └──┬───┘│└──┬──┘└─┬──┘│ └──┬──┘│ └───┬────┘└───┬────┘│
    │    │   │     │   │    │   │     │          │     │
    │    ▼   │     │   │    │   │     │          │     │
    │ ┌──────────┐ │   │    │   │     │          │     │
    ├►│  render  │◄┘   │    │   │     │          │     │
    │ └────┬─────┘     │    │   │     │          │     │
    │      │           │    │   │     │          │     │
    │      ▼           │    │   │     │          │     │
    │ ┌──────────┐     │   ┌┘   │     │          │     │
    │ │  import  │     │   │    │     │          │     │
    │ └────┬─────┘     │   │    │     │          │     │
    │      │           ▼   ▼    ▼     ▼          ▼     ▼
    │      │    ┌──────────────────────────────────────────┐
    │      │    │              flint-ecs                    │
    │      │    │  (hecs wrapper, stable IDs, hierarchy)   │
    │      │    └────────────────┬─────────────────────────┘
    │      │                    │
    │      │                    ▼
    │      │             ┌──────────────┐
    │      │             │ flint-schema │
    │      │             └──────┬───────┘
    │      │                    │
    │      ▼                    ▼
    │ ┌──────────┐  ┌─────────────────────────────────────┐
    │ │  asset   │  │            flint-core                │
    │ └────┬─────┘  │ (EntityId, Vec3, Transform, Hash)   │
    │      │        └─────────────────────────────────────┘
    │      │                    ▲
    └──────┴────────────────────┘
```

## Dependency Details

| Crate | Depends On | Depended On By |
|-------|-----------|----------------|
| `flint-core` | *(none)* | all other crates |
| `flint-schema` | core | ecs, constraint |
| `flint-ecs` | core, schema | scene, query, render, constraint, runtime, physics, viewer, player, cli |
| `flint-asset` | core | import, cli |
| `flint-import` | core, asset | render, viewer, cli, player |
| `flint-query` | core, ecs | constraint, cli |
| `flint-scene` | core, ecs, schema | viewer, player, cli |
| `flint-constraint` | core, ecs, schema, query | viewer, cli |
| `flint-render` | core, ecs, import | viewer, player, cli |
| `flint-runtime` | core, ecs | physics, player |
| `flint-physics` | core, ecs, runtime | player |
| `flint-viewer` | core, ecs, scene, schema, render, import, constraint | cli |
| `flint-player` | core, schema, ecs, scene, render, runtime, physics, import | *(binary entry point)* |
| `flint-cli` | all crates | *(binary entry point)* |

## Key Properties

**Acyclic.** The dependency graph has no cycles. This is enforced by Cargo and ensures clean compilation ordering.

**Layered.** Crates form clear layers:
1. **Core** --- fundamental types (`flint-core`)
2. **Schema** --- data definitions (`flint-schema`)
3. **Storage** --- entity and asset management (`flint-ecs`, `flint-asset`)
4. **Logic** --- query, scene, constraint, import
5. **Systems** --- render, runtime, physics
6. **Applications** --- viewer, player
7. **Interface** --- CLI binary (`flint-cli`), player binary (`flint-player`)

**Two entry points.** The CLI binary (`flint-cli`) serves scene authoring and validation workflows. The player binary (`flint-player`) serves interactive gameplay. Both share the same underlying crate hierarchy.

**Independent subsystems.** The constraint system, asset system, physics system, and render system don't depend on each other. This means you can build and test each subsystem in isolation.

## External Dependencies

Key third-party crates used across the workspace:

| Crate | Used By | Purpose |
|-------|---------|---------|
| `hecs` | flint-ecs | Underlying ECS implementation |
| `toml` | most crates | TOML parsing and serialization |
| `serde` | all crates | Serialization framework |
| `pest` | flint-query | PEG parser generator |
| `wgpu` | flint-render, flint-viewer, flint-player | GPU abstraction layer |
| `winit` | flint-render, flint-viewer, flint-runtime, flint-player | Window and input management |
| `rapier3d` | flint-physics | 3D physics simulation |
| `egui` | flint-viewer | Immediate-mode GUI framework |
| `clap` | flint-cli, flint-player | Command-line argument parsing |
| `thiserror` | all library crates | Error derive macros |
| `sha2` | flint-core, flint-asset | SHA-256 hashing |
| `gltf` | flint-import | glTF file parsing |
| `crossbeam` | flint-physics | Channel-based event collection (Rapier) |
