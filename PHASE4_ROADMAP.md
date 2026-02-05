# Phase 4: Interactive Runtime

Phase 4 transforms Flint from a scene viewer into an interactive game engine with real-time physics, player control, and eventually scripted gameplay.

## Stages

### Stage 1: Game Loop + Physics
- [x] `flint-runtime` crate — GameClock (fixed-timestep accumulator), InputState (keyboard/mouse with action bindings), EventBus, RuntimeSystem trait
- [x] `flint-physics` crate — Rapier 3D integration: PhysicsWorld, PhysicsSync (TOML component <-> Rapier bridge), CharacterController (kinematic first-person movement with gravity/jumping)
- [x] Camera first-person mode — CameraMode enum, `update_first_person()` on Camera (backward-compatible with orbit)
- [x] `flint-player` crate — standalone binary with full game loop: clock tick, fixed-step physics, character controller, first-person rendering
- [x] CLI `play` command — `flint play <scene> [--schemas] [--fullscreen]`
- [x] Physics schemas — `rigidbody.toml`, `collider.toml`, `character_controller.toml` components + `player.toml` archetype
- [x] Demo scene — `demo/phase4_runtime.scene.toml` (walkable tavern with floor/wall/furniture colliders)

### Stage 2: Audio
- [ ] `flint-audio` crate — Kira integration for game audio
- [ ] Spatial audio — 3D positioned sounds that attenuate with distance
- [ ] Ambient loops — background music and environment sounds
- [ ] Audio component schema — `audio_source.toml` with volume, loop, spatial settings
- [ ] Sound trigger events — play sounds on collision, action press, custom events

### Stage 3: Scripting
- [ ] `flint-script` crate — Rhai scripting engine integration
- [ ] Entity API — scripts can read/write entity components, spawn/despawn
- [ ] Event callbacks — `on_collision`, `on_trigger`, `on_action`, `on_update`
- [ ] Script component schema — `script.toml` with source path and enabled flag
- [ ] Hot-reload — watch script files for changes

### Stage 4: Integration
- [ ] Interactable component — `interactable.toml` with prompt text, interaction range
- [ ] Demo scene — tavern with openable doors (scripted), sound effects, ambient music
- [ ] Door script — opens/closes on E key press with sound and rotation animation
- [ ] Full game loop demo — walk around, interact with objects, hear sounds

## Architecture

```
flint-player / flint-cli play
    ├── flint-runtime    (GameClock, InputState, EventBus, RuntimeSystem)
    ├── flint-physics    (PhysicsWorld, PhysicsSync, CharacterController)
    ├── flint-render     (Camera with FirstPerson mode, SceneRenderer)
    ├── flint-ecs        (FlintWorld, DynamicComponents)
    └── flint-core       (EntityId, Transform, Vec3, FlintError)
```

## Running the Demo

```bash
# Standalone player binary
cargo run --bin flint-player -- demo/phase4_runtime.scene.toml --schemas schemas

# Via CLI
cargo run --bin flint -- play demo/phase4_runtime.scene.toml

# Controls
#   WASD     - Move
#   Mouse    - Look around
#   Space    - Jump
#   Shift    - Sprint
#   Escape   - Release cursor / Exit
#   F1       - Cycle debug rendering mode
#   F4       - Toggle shadows
#   F11      - Toggle fullscreen
```
