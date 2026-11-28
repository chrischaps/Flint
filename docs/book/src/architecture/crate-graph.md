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
  └──┬───┘│    (stable IDs, components, index)     │
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

The long arrow into `render` comes from the player (top right) and from `viewer` and `debug-ui`; the systems row beneath it (`script`, `physics`, `audio`, `animation`, `particles`) does not depend on `render`. Some edges are left out for legibility (every crate reaches `flint-core`; `render`, `animation`, `viewer` and `asset-gen` reach `import`; `render` reaches `scene`; `terrain` also feeds `debug-ui`), and so are nine crates: `living` (the living props' runtime, over `procgen`, under the player and the CLI), `rhythm` (over `music`, under `input-capture`, the player and the CLI), `procgen-ai` (under the CLI), `ai-core` (under `procgen-ai`, `asset-gen` and the CLI: what every paid AI call goes through), `android` (the Android entry, over the player), the small `wooze` and `geometry` layers just above `flint-core`, `pixels` (no dependencies, under `render`), and `audio-worklet`, a separate wasm with no engine dependency at all. The table below is complete: `tools/arch-analyzer` writes it from each crate's `Cargo.toml`, and CI fails a pull request that leaves it stale.

## Dependency Details

<!-- crate-table:begin -->
<!-- Written by tools/arch-analyzer from each Cargo.toml; don't edit by hand.
     Rewrite: cargo run -p flint-arch-analyzer -- --book. CI's crate-table job runs --check-book. -->

The workspace has 39 engine crates, 23 under `crates/` and 16 in packages (`packages/ai/crates/`, `packages/animation/crates/`, `packages/debug-hud/crates/`, `packages/flipbook/crates/`, `packages/living/crates/`, `packages/music/crates/`, `packages/procgen/crates/`, `packages/rhythm/crates/`, `packages/terrain/crates/`, `packages/vehicle/crates/`), plus one tool (`tools/arch-analyzer`). Rows run bottom-up: a crate's **tier** is the longest chain of engine crates beneath it, so every crate depends only on crates of a lower tier. Optional and target-only dependencies are marked; dev-dependencies are left out.

| Tier | Crate | Depends On | Depended On By |
|-----:|-------|-----------|----------------|
| 0 | `flint-ai-core` | *(none)* | asset-gen, procgen-ai |
| 0 | `flint-arch-analyzer` (`tools/arch-analyzer`) | *(none)* | *(none)* |
| 0 | `flint-audio-worklet` | *(none)* | *(none)* |
| 0 | `flint-core` | *(none)* | android, animation, asset, asset-gen, audio, cli, constraint, debug-ui, ecs, flipbook, geometry, import, input-capture, living, music, package, particles, physics, player, procgen, procgen-cli, query, render, rhythm, rhythm-game, runtime, scene, schema, script, terrain, vehicle, viewer, widgets, wooze |
| 0 | `flint-pixels` | *(none)* | player, render |
| 1 | `flint-asset` | core | asset-gen, cli, import, player, procgen (optional), procgen-cli, scene |
| 1 | `flint-schema` | core | android, cli, constraint, debug-ui, ecs, living (optional), package, player, procgen, render, scene, script, viewer, widgets |
| 2 | `flint-ecs` | core, schema | android, animation, audio, cli, constraint, debug-ui, flipbook, geometry, living (optional), music (optional), package, particles, physics, player, procgen (optional), procgen-cli, query, render, rhythm-game (optional), runtime, scene, script, terrain, vehicle (optional), viewer, wooze |
| 2 | `flint-import` | asset, core | animation, asset-gen, cli, living (optional), player, procgen, procgen-ai, procgen-cli, render, viewer |
| 3 | `flint-geometry` | core, ecs | cli, debug-ui, package, player, procgen, render, script, terrain, viewer |
| 3 | `flint-query` | core, ecs | cli, constraint, player |
| 3 | `flint-runtime` | core, ecs | animation, audio, cli, input-capture, living (optional), package, particles, physics, player, procgen-cli, rhythm-game (optional), script, vehicle (optional), viewer |
| 3 | `flint-scene` | asset, core, ecs, schema | android, asset-gen (optional), cli, debug-ui, living (optional), package, player, procgen, render, rhythm-game (optional), viewer, widgets |
| 3 | `flint-wooze` | core, ecs | living, procgen (optional) |
| 4 | `flint-audio` | core, ecs, runtime | music, player, rhythm-game (optional) |
| 4 | `flint-constraint` | core, ecs, query, schema | cli, viewer |
| 4 | `flint-physics` | core, ecs, runtime | cli, player, script, vehicle (optional) |
| 4 | `flint-render` | core, ecs, geometry, import, pixels, scene, schema | animation (optional), asset-gen (optional), cli, debug-ui, living (optional), package, particles, player, procgen (optional), procgen-cli, rhythm-game (optional), terrain (optional), viewer, widgets |
| 5 | `flint-particles` | core, ecs, render, runtime | cli, debug-ui, player, viewer |
| 5 | `flint-script` | core, ecs, geometry, physics, runtime, schema | animation (optional), cli, debug-ui (optional), flipbook (optional), living (optional), music (optional), package, player, rhythm-game (optional), terrain (optional), vehicle (optional) |
| 6 | `flint-package` | core, ecs, geometry, render, runtime, scene, schema, script | animation (optional), asset-gen (optional), cli, debug-ui, flipbook (optional), living (optional), music (optional), player, procgen (optional), procgen-cli, rhythm-game (optional), terrain (optional), vehicle (optional), viewer, widgets |
| 7 | `flint-animation` | core, ecs, import, package (optional), render (optional), runtime, script (optional) | cli, flipbook, player (optional) |
| 7 | `flint-music` | audio, core, ecs (optional), package (optional), script (optional) | cli (optional), input-capture, player (optional), rhythm, rhythm-game |
| 7 | `flint-vehicle` | core, ecs (optional), package (optional), physics (optional), runtime (optional), script (optional) | player (optional) |
| 7 | `flint-widgets` | core, package, render, scene, schema | cli, debug-ui, viewer |
| 8 | `flint-debug-ui` | core, ecs, geometry, package, particles, render, scene, schema, script (optional), widgets | living (optional), player (optional), rhythm-game (optional) |
| 8 | `flint-flipbook` | animation, core, ecs, package (optional), script (optional) | player (optional) |
| 8 | `flint-rhythm` | core, music | input-capture, rhythm-game |
| 8 | `flint-viewer` | constraint, core, ecs, geometry, import, package, particles, render, runtime, scene, schema, widgets | cli, living (optional), procgen-cli, terrain (optional) |
| 9 | `flint-input-capture` | core, music, rhythm, runtime | rhythm-game |
| 9 | `flint-terrain` | core, ecs, geometry, package (optional), render (optional), script (optional), viewer (optional) | cli (optional), player (optional), procgen (optional) |
| 10 | `flint-procgen` | asset (optional), core, ecs (optional), geometry, import, package (optional), render (optional), scene, schema, terrain (optional), wooze (optional) | asset-gen, living, player (optional), procgen-ai, procgen-cli |
| 10 | `flint-rhythm-game` | audio (optional), core, debug-ui (optional), ecs (optional), input-capture, music, package (optional), render (optional), rhythm, runtime (optional), scene (optional), script (optional) | cli (optional), player (optional) |
| 11 | `flint-procgen-ai` | ai-core, import, procgen | asset-gen (optional) |
| 11 | `flint-procgen-cli` | asset, core, ecs, import, package, procgen, render, runtime, viewer | asset-gen (optional), cli (optional), living (optional) |
| 12 | `flint-asset-gen` | ai-core, asset, core, import, package (optional), procgen, procgen-ai (optional), procgen-cli (optional), render (optional), scene (optional) | cli (optional) |
| 12 | `flint-living` | core, debug-ui (optional), ecs (optional), import (optional), package (optional), procgen, procgen-cli (optional), render (optional), runtime (optional), scene (optional), schema (optional), script (optional), viewer (optional), wooze | cli (optional), player (optional) |
| 13 | `flint-player` | animation (optional), asset, audio, core, debug-ui (optional), ecs, flipbook (optional), geometry, import, living (optional), music (optional), package, particles, physics, pixels, procgen (optional), query, render, rhythm-game (optional), runtime, scene, schema, script, terrain (optional), vehicle (optional) | android, cli |
| 14 | `flint-android` | core, ecs, player, scene, schema | *(none)* |
| 14 | `flint-cli` | animation, asset, asset-gen (optional), constraint, core, ecs, geometry, import, living (optional), music (optional), package, particles, physics, player, procgen-cli (optional), query, render, rhythm-game (optional), runtime, scene, schema, script, terrain (optional), viewer, widgets | *(none)* |

<!-- crate-table:end -->

## Key Properties

**Acyclic.** The dependency graph has no cycles. This is enforced by Cargo and ensures clean compilation ordering.

**Layered.** Crates form clear layers:
1. **Core** --- fundamental types (`flint-core`), with the maths only some features read just above it (`flint-wooze`, `flint-geometry`, #754), so an edit there rebuilds only their readers
2. **Schema** --- data definitions (`flint-schema`)
3. **Storage** --- entity and asset management (`flint-ecs`, `flint-asset`)
4. **Logic** --- query, scene, constraint, import, asset-gen, procgen, procgen-ai, music
5. **Systems** --- render, terrain, runtime, physics, audio, input-capture, animation, particles, script
6. **Tooling overlays** --- debug-ui (panels over the renderer), viewer
7. **Applications** --- player, android
8. **Interface** --- CLI binary (`flint-cli`), player binary (`flint-player`)

**Two entry points.** The CLI binary (`flint-cli`) serves scene authoring, validation, and the rhythm-game tooling (`flint music`). The player binary (`flint-player`) serves interactive gameplay, and `flint-cli` embeds it so `flint play` and `flint-player` run the same code. Both share the same underlying crate hierarchy.

**Mostly independent subsystems.** The constraint, asset, physics, audio, particles, asset generation, and render systems don't depend on each other. The exceptions are deliberate: `flint-script` depends on `flint-physics` (raycasts, camera direction) and `flint-animation` (layer and sequence bindings); `flint-input-capture` depends on `flint-music` for the `InputEvent` type and clock bridge, and on `flint-runtime` for the gamepad backend it shares with the player; `flint-render` depends on `flint-terrain` for the shared vertex format and on `flint-scene` for the post-process and environment definitions; and `flint-debug-ui` depends on `flint-render` because its panels mirror and write through to renderer state. Most subsystems can still be built and tested in isolation.

## External Dependencies

Key third-party crates used across the workspace:

| Crate | Used By | Purpose |
|-------|---------|---------|
| `toml` | most crates | TOML parsing and serialization |
| `serde` | all crates | Serialization framework |
| `pest` | flint-query | PEG parser generator |
| `wgpu` | flint-render, flint-viewer, flint-player | GPU abstraction layer |
| `winit` | flint-render, flint-viewer, flint-runtime, flint-player | Window and input management |
| `rapier3d` | flint-physics | 3D physics simulation |
| `kira` | flint-audio, flint-music, flint-rhythm | Spatial audio engine; stem playback and the stem mixer |
| `glam` | flint-audio | Vec3/Quat types for Kira spatial positioning (via mint interop) |
| `symphonia` | flint-music | Stem decoding for validation and offline rendering |
| `hound` | flint-music | WAV output for `flint music render` |
| `egui` | flint-viewer, flint-player, flint-cli, flint-debug-ui | Immediate-mode GUI framework (inspector, HUD, debug panels) |
| `egui-snarl` | flint-cli | Node graph widget for the texture pipeline editor |
| `rfd` | flint-cli | Native file dialogs in the editors |
| `clap` | flint-cli, flint-player | Command-line argument parsing |
| `thiserror` | all library crates | Error derive macros |
| `sha2` | flint-core, flint-asset | SHA-256 hashing |
| `gltf` | flint-import | glTF file parsing (meshes, materials, skins, animations) |
| `crossbeam` | flint-physics | Channel-based event collection (Rapier) |
| `rhai` | flint-script | Embedded scripting language |
| `gilrs` | flint-runtime (optional, `gamepad`) | Gamepad input (buttons, axes, multi-controller) and a script's `rumble()`; the player and the rhythm capture read it through `flint_runtime::pad` |
| `gilrs-core` | flint-input-capture | Direct 1 kHz polling and XInput rumble |
| `noise` | flint-procgen, flint-terrain | Perlin / simplex / Worley noise |
| `rand`, `rand_chacha` | flint-procgen, flint-procgen-ai, flint-terrain | Seeded, reproducible randomness |
| `syn` | flint-arch-analyzer | Rust source parsing for the architecture analyzer |
| `ureq` | flint-asset-gen | HTTP client for AI provider APIs |
| `uuid` | flint-asset-gen | Unique job identifiers |
