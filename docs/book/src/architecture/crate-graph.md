# Crate Dependency Graph

This page shows how Flint's crates depend on each other. Dependencies flow downward --- higher crates depend on lower ones, never the reverse.

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

The long arrow into `render` comes from the player (top right) and from `viewer` and `debug-ui`; the systems row beneath it (`script`, `physics`, `audio`, `animation`, `particles`) does not depend on `render`. Some edges are left out for legibility (every crate reaches `flint-core`; `render`, `animation`, `viewer` and `asset-gen` reach `import`; `render` reaches `scene`; `terrain` also feeds `debug-ui`). The table below is complete: `tools/arch-analyzer` writes it from each crate's `Cargo.toml`, and CI fails a pull request that leaves it stale.

## Dependency Details

<!-- crate-table:begin -->
<!-- Written by tools/arch-analyzer from each Cargo.toml; don't edit by hand.
     Rewrite: cargo run -p flint-arch-analyzer -- --book. CI's crate-table job runs --check-book. -->

The workspace has 27 crates under `crates/`, plus one tool (`tools/arch-analyzer`). Rows run bottom-up: a crate's **tier** is the longest chain of engine crates beneath it, so every crate depends only on crates of a lower tier. Optional and target-only dependencies are marked; dev-dependencies are left out.

| Tier | Crate | Depends On | Depended On By |
|-----:|-------|-----------|----------------|
| 0 | `flint-arch-analyzer` (`tools/arch-analyzer`) | *(none)* | *(none)* |
| 0 | `flint-audio-worklet` | *(none)* | *(none)* |
| 0 | `flint-core` | *(none)* | animation, asset, asset-gen, audio, cli, constraint, debug-ui, ecs, import, input-capture, music, particles, physics, player, procgen, query, render, runtime, scene, schema, script, terrain, viewer |
| 1 | `flint-asset` | core | asset-gen, cli, import, player |
| 1 | `flint-music` | core | cli, input-capture, player |
| 1 | `flint-schema` | core | android, cli, constraint, ecs, player, scene, viewer |
| 1 | `flint-terrain` | core | cli, debug-ui, player, render, script |
| 2 | `flint-ecs` | core, schema | android, animation, audio, cli, constraint, particles, physics, player, query, render, runtime, scene, script, viewer |
| 2 | `flint-import` | asset, core | animation, asset-gen, cli, player, procgen, render, viewer |
| 2 | `flint-input-capture` | core, music | cli, player |
| 3 | `flint-procgen` | core, import | asset-gen, cli, player, procgen-ai |
| 3 | `flint-query` | core, ecs | cli, constraint |
| 3 | `flint-runtime` | core, ecs | animation, audio, cli, particles, physics, player, script |
| 3 | `flint-scene` | core, ecs, schema | android, cli, debug-ui, player, render, viewer |
| 4 | `flint-animation` | core, ecs, import, runtime | cli, player, script |
| 4 | `flint-asset-gen` | asset, core, import, procgen | cli |
| 4 | `flint-audio` | core, ecs, runtime | player |
| 4 | `flint-constraint` | core, ecs, query, schema | cli, viewer |
| 4 | `flint-particles` | core, ecs, runtime | cli, player, render, viewer |
| 4 | `flint-physics` | core, ecs, runtime | cli, player, script |
| 4 | `flint-procgen-ai` | procgen | cli |
| 5 | `flint-render` | core, ecs, import, particles, scene, terrain | cli, debug-ui, player, viewer |
| 5 | `flint-script` | animation, core, ecs, physics, runtime, terrain | cli, player |
| 6 | `flint-debug-ui` | core, render, scene, terrain | cli, player (optional), viewer |
| 7 | `flint-player` | animation, asset, audio, core, debug-ui (optional), ecs, import, input-capture, music, particles, physics, procgen, render, runtime, scene, schema, script, terrain | android, cli |
| 7 | `flint-viewer` | constraint, core, debug-ui, ecs, import, particles, render, scene, schema | cli |
| 8 | `flint-android` | ecs, player, scene, schema | *(none)* |
| 8 | `flint-cli` | animation, asset, asset-gen, constraint, core, debug-ui, ecs, import, input-capture, music, particles, physics, player, procgen, procgen-ai, query, render, runtime, scene, schema, script, terrain, viewer | *(none)* |

<!-- crate-table:end -->

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
