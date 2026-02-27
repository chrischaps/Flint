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
- [x] `flint-audio` crate — Kira 0.11 integration: AudioEngine, AudioSync, AudioTrigger, AudioSystem (RuntimeSystem impl)
- [x] Spatial audio — 3D positioned sounds with distance attenuation via Kira SpatialTrackHandle
- [x] Ambient loops — non-spatial looping sounds on main track (tavern ambience)
- [x] Audio component schemas — `audio_source.toml` (file, volume, pitch, loop, spatial, distance), `audio_listener.toml`, `audio_trigger.toml`
- [x] Sound trigger events — collision events generate AudioCommands, event-driven playback
- [x] Graceful degradation — `Option<AudioManager>` pattern, silent operation in headless/CI
- [x] Demo audio — CC0 OGG assets: fire crackle, ambient tavern, door open, glass clinks
- [x] PlayerApp integration — audio system wired into game loop tick with listener tracking

### Stage 3: Animation
- [x] `flint-animation` crate — animation playback engine with two tiers:

#### Tier 1: Property Animation
- [x] `AnimationClip` — keyframe tracks targeting any transform property (position, rotation, scale) + custom float fields
- [x] Interpolation modes — Step, Linear, CubicSpline (matching glTF spec)
- [x] `AnimationPlayer` — drives clip playback with play/pause/stop, looping, speed control, event firing
- [x] TOML-defined clips — `.anim.toml` files in `demo/animations/` (door swing, platform bob)
- [x] `RuntimeSystem` integration — AnimationSystem ticks each frame via GameClock delta, writes updated transforms back to ECS
- [x] `animator` component schema — clip name, playing, autoplay, loop, speed
- [x] `AnimationSync` — bridges ECS `animator` components to playback states, auto-discovers new entities
- [x] Demo — bobbing platform (4s looping) and door swing (0.8s) in phase4_runtime scene
- [x] 15 unit tests — sampler boundary cases, interpolation modes, player advance, event firing, TOML parsing

#### Tier 2: Skeletal Animation
- [x] glTF skin import — extract joint hierarchies, inverse bind matrices, and per-vertex bone weights/indices from glTF files via `flint-import`
- [x] `Skeleton` type — bone tree with bind-pose and current-pose matrices, joint name lookup
- [x] `ImportedMesh` extension — add `joint_indices: Vec<[u32; 4]>` and `joint_weights: Vec<[f32; 4]>` fields
- [x] Vertex format extension — add `@location(4) joint_indices` and `@location(5) joint_weights` via separate `SkinnedVertex` type and WGSL shader input
- [x] GPU skinning — bone matrix storage buffer, vertex shader skinning pass that transforms positions/normals by weighted bone matrices
- [x] Skinned render pipeline — separate pipeline variant for skinned vs. static meshes (avoids cost on static geometry)
- [x] glTF animation clip import — extract animation channels (translation, rotation, scale per joint) with keyframe samplers
- [x] Skinned shadow mapping — dedicated `vs_skinned_shadow` entry point applies bone transforms before depth rendering

#### Animation Blending & Control
- [x] Crossfade — blend between two clips over a duration (e.g., idle → walk transition) via `blend_target`/`blend_duration` fields
- [ ] Additive blending — layer animations (walk + wave hand)
- [x] `animator` component schema — `animator.toml` with current clip, playback state, speed, blend settings
- [x] `skeleton` component schema — `skeleton.toml` with glTF skin reference
- [x] Animation events — fire EventBus events at specific keyframe times (e.g., footstep at frame 12)
- [ ] CLI introspection — `flint query` can inspect animation state (current clip, time, blend weight)

### Stage 4: Scripting
- [x] `flint-script` crate — Rhai scripting engine integration (ScriptEngine, ScriptSync, ScriptSystem)
- [x] Entity API — scripts can read/write entity components, spawn/despawn, get/set position/rotation, distance
- [x] Event callbacks — `on_init`, `on_update(dt)`, `on_collision(other)`, `on_trigger_enter/exit(entity)`, `on_action(name)`, `on_interact`
- [x] Input API — `is_action_pressed()`, `is_action_just_pressed()`, `mouse_delta_x/y()`, `delta_time()`, `total_time()`
- [x] Audio API — `play_sound()`, `play_sound_at()`, `stop_sound()` via ScriptCommand pattern
- [x] Animation API — `play_clip()`, `stop_clip()`, `blend_to()`, `set_anim_speed()` (direct ECS writes)
- [x] Math API — `clamp`, `lerp`, `random`, `random_range`, `sin`, `cos`, `abs`, `sqrt`, `floor`, `ceil`, `min`, `max`
- [x] Event firing — `fire_event()`, `fire_event_data()` → GameEvent::Custom
- [x] Script component schema — `script.toml` with source path and enabled flag
- [x] Hot-reload — file timestamp checking each frame; preserves Scope on recompile, keeps old AST on error
- [x] Demo scripts — `door_interact.rhai` (toggle door on E + proximity), `torch_flicker.rhai` (organic light/color animation)
- [x] 26 unit tests — engine creation, compilation, callback detection, entity API, input, events, commands, math, hot-reload

### Stage 5: Integration
- [x] Interactable component — `interactable.toml` with prompt text, interaction range, type, enabled flag
- [x] HUD overlay — egui-based crosshair and interaction prompt with fade in/out
- [x] Enhanced door script — animation clip-driven open/close with locked-door sound
- [x] NPC behavior scripts — bartender (alert/greeting), patrons (breathing idle), mysterious stranger (freeze effect)
- [x] Player footstep sounds — movement-triggered spatial audio at walk/sprint intervals
- [x] Ambient event system — random micro-events (glass clinks, fire crackle variations)
- [x] Scene polish — ceiling, candle flicker, asymmetric torch lighting, NPC positioning for storytelling
- [x] Audio auto-loading — preload all audio files from `audio/` directory for script-triggered sounds
- [x] Math API extension — `atan2()` for NPC facing calculations
- [x] Full game loop demo — walk around, interact with objects/NPCs, hear sounds, see animations

## Architecture

```
flint-player / flint-cli play
    ├── flint-runtime    (GameClock, InputState, EventBus, RuntimeSystem)
    ├── flint-audio      (AudioEngine, AudioSync, AudioTrigger — Kira spatial audio)
    ├── flint-physics    (PhysicsWorld, PhysicsSync, CharacterController)
    ├── flint-script     (ScriptEngine, ScriptSync, ScriptSystem — Rhai scripting)
    ├── flint-animation  (AnimationPlayer, AnimationClip, Skeleton, GPU skinning)
    ├── flint-render     (Camera with FirstPerson mode, SceneRenderer, skinned mesh pipeline)
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
#   E        - Interact (triggers on_interact in nearby scripts)
#   Escape   - Release cursor / Exit
#   F1       - Cycle debug rendering mode
#   F4       - Toggle shadows
#   F11      - Toggle fullscreen
```
