# Crate Dependency Graph

This page shows how Flint's eighteen crates depend on each other. Dependencies flow downward --- higher crates depend on lower ones, never the reverse.

## Dependency Diagram

```
          ┌─────────────┐                ┌──────────────┐
          │  flint-cli  │                │ flint-player │
          │  (binary)   │                │   (binary)   │
          └──────┬──────┘                └──────┬───────┘
                 │                              │
    ┌────┬───┬──┴──┬────┬────┬─────┐   ┌───┬───┴───┬───┬───┬───┬───┐
    │    │   │     │    │    │     │   │   │       │   │   │   │   │
    ▼    │   ▼     ▼    ▼    ▼     ▼   ▼   ▼       ▼   │   │   │   │
 ┌──────┐│┌─────┐┌────┐│ ┌─────┐┌────────┐┌────────┐┌────────┐│   │
 │viewer│││scene││qry ││ │const││asset-gen││runtime ││physics ││   │
 └──┬───┘│└──┬──┘└─┬──┘│ └──┬──┘└───┬────┘└───┬────┘└───┬────┘│   │
    │    │   │     │   │    │       │          │         │     │   │
    │    ▼   │     │   │    │       │          │         │     ▼   │
    │ ┌──────────┐ │   │    │       │          │         │  ┌─────────┐
    ├►│  render  │◄┘   │    │       │          │         ├─►│ script  │
    │ └────┬─────┘     │    │       │          │         │  └────┬────┘
    │      │           │    │       │          │         │       │
    │      │           │    │       │          │         ▼       ▼
    │      │           │    │       │          │      ┌─────────────┐
    │      │           │    │       │          │      │audio  anim  │
    │      ▼           │    │       │          │      └──────┬──────┘
    │ ┌──────────┐     │   ┌┘      │          │             │
    │ │  import  │     │   │       │          │             │
    │ └────┬─────┘     │   │       │          │             │
    │      │           ▼   ▼       ▼          ▼             ▼
    │      │    ┌──────────────────────────────────────────────────┐
    │      │    │              flint-ecs                            │
    │      │    │  (hecs wrapper, stable IDs, hierarchy)           │
    │      │    └────────────────┬─────────────────────────────────┘
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
| `flint-ecs` | core, schema | scene, query, render, constraint, runtime, physics, audio, animation, viewer, player, cli |
| `flint-asset` | core | import, cli |
| `flint-import` | core, asset | render, animation, viewer, cli, player |
| `flint-query` | core, ecs | constraint, cli |
| `flint-scene` | core, ecs, schema | viewer, player, cli |
| `flint-constraint` | core, ecs, schema, query | viewer, cli |
| `flint-render` | core, ecs, import | viewer, player, cli |
| `flint-runtime` | core, ecs | physics, audio, animation, player |
| `flint-physics` | core, ecs, runtime | script, player |
| `flint-audio` | core, ecs, runtime | player |
| `flint-animation` | core, ecs, import, runtime | player |
| `flint-script` | core, ecs, runtime, physics | player |
| `flint-asset-gen` | core, asset, import | cli |
| `flint-viewer` | core, ecs, scene, schema, render, import, constraint | cli |
| `flint-player` | core, schema, ecs, scene, render, runtime, physics, audio, animation, script, import, asset | *(binary entry point)* |
| `flint-cli` | all crates | *(binary entry point)* |

## Key Properties

**Acyclic.** The dependency graph has no cycles. This is enforced by Cargo and ensures clean compilation ordering.

**Layered.** Crates form clear layers:
1. **Core** --- fundamental types (`flint-core`)
2. **Schema** --- data definitions (`flint-schema`)
3. **Storage** --- entity and asset management (`flint-ecs`, `flint-asset`)
4. **Logic** --- query, scene, constraint, import, asset-gen
5. **Systems** --- render, runtime, physics, audio, animation, script
6. **Applications** --- viewer, player
7. **Interface** --- CLI binary (`flint-cli`), player binary (`flint-player`)

**Two entry points.** The CLI binary (`flint-cli`) serves scene authoring and validation workflows. The player binary (`flint-player`) serves interactive gameplay. Both share the same underlying crate hierarchy.

**Mostly independent subsystems.** The constraint, asset, physics, audio, animation, asset generation, and render systems don't depend on each other. The one exception is `flint-script`, which depends on `flint-physics` for raycasting and camera direction access. This means most subsystems can be built and tested in isolation.

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
| `kira` | flint-audio | Spatial audio engine |
| `glam` | flint-audio | Vec3/Quat types for Kira spatial positioning (via mint interop) |
| `egui` | flint-viewer | Immediate-mode GUI framework |
| `clap` | flint-cli, flint-player | Command-line argument parsing |
| `thiserror` | all library crates | Error derive macros |
| `sha2` | flint-core, flint-asset | SHA-256 hashing |
| `gltf` | flint-import | glTF file parsing (meshes, materials, skins, animations) |
| `crossbeam` | flint-physics | Channel-based event collection (Rapier) |
| `rhai` | flint-script | Embedded scripting language |
| `gilrs` | flint-player | Gamepad input (buttons, axes, multi-controller) |
| `ureq` | flint-asset-gen | HTTP client for AI provider APIs |
| `uuid` | flint-asset-gen | Unique job identifiers |
