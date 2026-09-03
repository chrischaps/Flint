# Crate Dependency Graph

This page shows how Flint's twenty-six crates (plus the `tools/arch-analyzer` workspace member) depend on each other. Dependencies flow downward --- higher crates depend on lower ones, never the reverse.

## Dependency Diagram

```
                     ┌─────────────┐
                     │  flint-cli  │  (binary; also embeds the player)
                     └──────┬──────┘
                            │
      ┌──────┬──────┬───────┼────────┬──────────┬───────────┐
      ▼      ▼      ▼       ▼        ▼          ▼           ▼
  ┌──────┐┌─────┐┌──────┐┌────────┐┌────────┐┌───────┐┌────────────┐
  │viewer││query││const ││asset-gen││procgen ││ music ││flint-player│ (binary)
  └──┬───┘└──┬──┘└──┬───┘└───┬────┘└───┬────┘└──┬────┘└─────┬──────┘
     │       │      │        │         │        │           │
     ▼       │      │        │         │        ▼           │
  ┌────────┐ │      │        │         │ ┌─────────────┐    │
  │debug-ui│◄┼──────┼────────┼─────────┼─┤input-capture│◄───┤  (player → debug-ui is optional)
  └──┬─────┘ │      │        │         │ └─────────────┘    │
     │       │      │        │         │                    │
     ▼       │      │        │         │   ┌────────┬───────┼────────┬──────────┐
  ┌────────┐ │      │        │         │   ▼        ▼       ▼        ▼          ▼
  │ render │◄┼──────┼────────┼─────────┼───────────────────────────────────────────┐
  └──┬──┬──┘ │      │        │         │ ┌──────┐┌───────┐┌────────┐┌─────────┐┌─────────┐
     │  │    │      │        │         │ │script││physics││ audio  ││animation││particles│
     │  ▼    │      │        │         │ └──┬───┘└───┬───┘└───┬────┘└────┬────┘└────┬────┘
  ┌────────┐ │      │        │         │    └────────┴────────┴──────────┴──────────┘
  │terrain │ │      │        │         │                       │
  └───┬────┘ │      │        │         │                       ▼
      │      │      │        │         │                 ┌───────────┐
      ▼      ▼      ▼        ▼         ▼                 │  runtime  │
  ┌──────┐┌────────────────────────────────────────┐     └─────┬─────┘
  │scene ││                 flint-ecs               │◄──────────┘
  └──┬───┘│      (hecs wrapper, stable IDs)        │
     │    └──────────────────┬──────────────────────┘
     │                       ▼
     │                ┌──────────────┐        ┌────────┐        ┌────────┐
     └───────────────►│ flint-schema │        │ import │───────►│ asset  │
                      └──────┬───────┘        └───┬────┘        └───┬────┘
                             ▼                    ▼                 ▼
                      ┌─────────────────────────────────────────────┐
                      │                  flint-core                  │
                      │       (EntityId, Vec3, Transform, Hash)      │
                      └─────────────────────────────────────────────┘
```

The long arrow into `render` comes from the player (top right) and from `viewer` and `debug-ui`; the systems row beneath it (`script`, `physics`, `audio`, `animation`, `particles`) does not depend on `render`. Some edges are left out for legibility (every crate reaches `flint-core`; `render`, `animation`, `viewer` and `asset-gen` reach `import`; `render` reaches `scene`; `terrain` also feeds `debug-ui`). The table below is complete.

## Dependency Details

| Crate | Depends On | Depended On By |
|-------|-----------|----------------|
| `flint-core` | *(none)* | all other crates |
| `flint-schema` | core | ecs, scene, constraint, viewer, player, cli, android |
| `flint-ecs` | core, schema | scene, query, render, constraint, runtime, physics, audio, animation, particles, script, viewer, player, cli, android |
| `flint-asset` | core | import, asset-gen, player, cli |
| `flint-import` | core, asset | render, animation, asset-gen, viewer, player, cli |
| `flint-query` | core, ecs | constraint, cli |
| `flint-scene` | core, ecs, schema | render, debug-ui, viewer, player, cli, android |
| `flint-constraint` | core, ecs, schema, query | viewer, cli |
| `flint-terrain` | core | render, debug-ui, player, cli |
| `flint-render` | core, ecs, scene, import, terrain | debug-ui, viewer, player, cli |
| `flint-debug-ui` | core, render, terrain, scene | viewer, player (optional, `debug-hud` feature) |
| `flint-runtime` | core, ecs | physics, audio, animation, particles, script, player, cli |
| `flint-physics` | core, ecs, runtime | script, player, cli |
| `flint-audio` | core, ecs, runtime | player |
| `flint-music` | core | input-capture, player, cli |
| `flint-input-capture` | core, music | player, cli |
| `flint-animation` | core, ecs, import, runtime | script, player, cli |
| `flint-particles` | core, ecs, runtime | player |
| `flint-script` | core, ecs, runtime, physics, animation | player |
| `flint-asset-gen` | core, asset, import | cli |
| `flint-procgen` | core | procgen-ai, player, cli |
| `flint-procgen-ai` | procgen | *(tool-time only; not a default member)* |
| `flint-viewer` | core, ecs, scene, schema, render, debug-ui, import, constraint | cli |
| `flint-player` | core, asset, schema, ecs, scene, render, runtime, physics, import, audio, music, input-capture, animation, particles, terrain, script, procgen, debug-ui (optional) | cli, android |
| `flint-android` | player, scene, schema, ecs | *(binary entry point, excluded from default members)* |
| `flint-cli` | core, schema, ecs, scene, query, render, constraint, asset, asset-gen, import, animation, runtime, physics, player, terrain, procgen, viewer, music, input-capture | *(binary entry point)* |
| `flint-arch-analyzer` (`tools/`) | *(no engine crates)* | *(standalone tool; not a default member)* |

## Key Properties

**Acyclic.** The dependency graph has no cycles. This is enforced by Cargo and ensures clean compilation ordering.

**Layered.** Crates form clear layers:
1. **Core** --- fundamental types (`flint-core`)
2. **Schema** --- data definitions (`flint-schema`)
3. **Storage** --- entity and asset management (`flint-ecs`, `flint-asset`)
4. **Logic** --- query, scene, constraint, import, asset-gen, procgen, procgen-ai, music
5. **Systems** --- render, terrain, runtime, physics, audio, input-capture, animation, particles, script
6. **Tooling overlays** --- debug-ui (panels over the renderer), viewer
7. **Applications** --- player, android
8. **Interface** --- CLI binary (`flint-cli`), player binary (`flint-player`)

**Two entry points.** The CLI binary (`flint-cli`) serves scene authoring, validation, and the rhythm-game tooling (`play-chart`, `replay-chart`, `render-suite`). The player binary (`flint-player`) serves interactive gameplay, and `flint-cli` embeds it so `flint play` and `flint-player` run the same code. Both share the same underlying crate hierarchy.

**Mostly independent subsystems.** The constraint, asset, physics, audio, particles, asset generation, and render systems don't depend on each other. The exceptions are deliberate: `flint-script` depends on `flint-physics` (raycasts, camera direction) and `flint-animation` (layer and sequence bindings); `flint-input-capture` depends on `flint-music` for the `InputEvent` type and clock bridge; `flint-render` depends on `flint-terrain` for the shared vertex format and on `flint-scene` for the post-process and environment definitions; and `flint-debug-ui` depends on `flint-render` because its panels mirror and write through to renderer state. Most subsystems can still be built and tested in isolation.

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
| `kira` | flint-audio, flint-music | Spatial audio engine; stem playback and the six-bus mixer |
| `glam` | flint-audio | Vec3/Quat types for Kira spatial positioning (via mint interop) |
| `symphonia` | flint-music | Stem decoding for validation and offline rendering |
| `hound` | flint-music | WAV output for `flint render-suite` |
| `egui` | flint-viewer, flint-player, flint-cli, flint-debug-ui | Immediate-mode GUI framework (inspector, HUD, debug panels) |
| `egui-snarl` | flint-cli | Node graph widget for the texture pipeline editor |
| `rfd` | flint-cli | Native file dialogs in the editors |
| `clap` | flint-cli, flint-player | Command-line argument parsing |
| `thiserror` | all library crates | Error derive macros |
| `sha2` | flint-core, flint-asset | SHA-256 hashing |
| `gltf` | flint-import | glTF file parsing (meshes, materials, skins, animations) |
| `crossbeam` | flint-physics | Channel-based event collection (Rapier) |
| `rhai` | flint-script | Embedded scripting language |
| `gilrs` | flint-player, flint-input-capture | Gamepad input (buttons, axes, multi-controller) |
| `gilrs-core` | flint-input-capture | Direct 1 kHz polling and XInput rumble |
| `noise` | flint-procgen, flint-terrain | Perlin / simplex / Worley noise |
| `rand`, `rand_chacha` | flint-procgen, flint-procgen-ai, flint-terrain | Seeded, reproducible randomness |
| `bimap` | flint-ecs | The `EntityId` ↔ hecs `Entity` map |
| `syn` | flint-arch-analyzer | Rust source parsing for the architecture analyzer |
| `ureq` | flint-asset-gen | HTTP client for AI provider APIs |
| `uuid` | flint-asset-gen | Unique job identifiers |
