# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Flint is a CLI-first, AI-agent-optimized 3D game engine written in Rust. The core thesis inverts traditional engine design: the primary interface is CLI and code, with visual tools focused on *validating* results rather than *creating* them. Currently through Phase 4 Stage 3 (Animation) with property tween animation, skeletal/glTF animation with GPU skinning, spatial audio, PBR rendering from Phase 3, constraints from Phase 2, and interactive first-person gameplay.

## Build & Development Commands

```bash
cargo build                    # Debug build
cargo build --release          # Release build
cargo run --bin flint -- <cmd> # Run CLI (e.g., cargo run --bin flint -- serve demo/showcase.scene.toml --watch)
cargo run --bin flint -- play demo/phase4_runtime.scene.toml  # Play a scene with first-person controls
cargo run --bin flint-player -- demo/phase4_runtime.scene.toml --schemas schemas  # Standalone player
cargo test                     # Run all tests (135 unit tests across crates)
cargo test -p flint-core       # Test a single crate
cargo clippy                   # Lint
cargo fmt --check              # Check formatting
```

## Architecture

16-crate Cargo workspace with clear dependency layering:

```
flint-cli           CLI binary (clap derive). Commands: init, entity, scene, query, schema, serve, play, validate, asset, render
flint-player        Standalone player binary with game loop, physics, audio, animation, first-person controls
  ├── flint-animation Property tween animation: AnimationClip, AnimationPlayer, AnimationSync, AnimationSystem
  ├── flint-audio    Kira spatial audio: AudioEngine, AudioSync, AudioTrigger, AudioSystem
  ├── flint-physics  Rapier 3D integration: PhysicsWorld, PhysicsSync, CharacterController
  ├── flint-runtime  Game loop infrastructure: GameClock, InputState, EventBus, RuntimeSystem trait
  ├── flint-viewer   egui-based GUI inspector
  ├── flint-import   glTF model importer
  ├── flint-asset    Content-addressed asset catalog
  ├── flint-constraint Constraint evaluation + auto-fix
  ├── flint-query    PEG query language (pest parser). Syntax: "entities where archetype == 'door'"
  ├── flint-scene    TOML scene serialization/deserialization with load/save
  ├── flint-render   wgpu-based PBR renderer (winit 0.30 ApplicationHandler, wgpu 23, orbit + first-person camera)
  ├── flint-ecs      hecs wrapper with stable EntityId mapping (BiMap), named entities, parent/child
  ├── flint-schema   Component/archetype schema system with TOML loading and runtime validation
  └── flint-core     Fundamental types: EntityId, ContentHash, Transform, Vec3, Color, FlintError
```

## Key Design Decisions

- **Dynamic components** stored as `toml::Value` (not Rust types) so archetypes are defined at runtime in schema TOML files
- **Stable EntityIds** via atomic counter (never recycled), persisted across save/load with counter adjustment to prevent collisions
- **Scene format is TOML** — human-readable, diffable, structured as `[scene]` metadata + `[entities.<name>]` sections
- **Schemas live in `schemas/`** directory — `components/*.toml` define field types and constraints, `archetypes/*.toml` bundle components with defaults
- **Query language** uses pest PEG parser (`grammar.pest`). Supports: `==`, `!=`, `contains`, `>`, `<`, `>=`, `<=` on entity fields
- **Scene reload** is full re-parse (not incremental) to avoid borrow checker issues with `Mutex<ViewerState>`
- **Physics uses Rapier 3D** via `flint-physics` crate — kinematic character controller for player, static bodies for world geometry
- **Game loop** uses fixed-timestep accumulator pattern (1/60s default) for deterministic physics
- **Camera modes** — `CameraMode::Orbit` (scene viewer default) and `CameraMode::FirstPerson` (player) share the same view/projection math
- **Audio uses Kira** via `flint-audio` crate — spatial 3D audio with distance attenuation, non-spatial ambient loops, event-driven triggers
- **Animation system** via `flint-animation` crate — property tweens (Tier 1) with Step/Linear/CubicSpline interpolation, `.anim.toml` clip files; skeletal animation (Tier 2) with glTF skin/joint import, GPU vertex skinning, bone matrix computation, crossfade blending; `animator` + `skeleton` component schemas, plays in `update()` (variable-rate for smooth interpolation)

## Technical Gotchas

- `toml::toml!` macro produces `Map<String, Value>` not `Value` — needs `.into()` in tests
- wgpu v23 `Instance::new()` takes owned `InstanceDescriptor`, not a reference
- winit v0.30 uses `ApplicationHandler` trait with `run_app()` — no direct event loop matching
- `FlintError` uses `thiserror`; crate-level `Result<T>` aliases throughout
- Rapier v0.22 character controller types are in `rapier3d::control`, not `rapier3d::prelude`
- `DeviceEvent::MouseMotion` provides raw mouse delta for first-person camera (independent of cursor position)
- Rapier's `ChannelEventCollector` requires `crossbeam` channels for collision events
- Kira v0.11 uses `Decibels(f32)` tuple struct, not `from_amplitude`; `PlaybackRate(f64)` likewise
- Kira uses `glam::Vec3`/`glam::Quat` for listener/track positioning via `mint` interop
- `AudioManager::new()` may fail in headless/CI — always wrap in `Option` for graceful degradation
- Animation clips use `serde` derive on all types — `TrackTarget` uses `#[serde(tag = "type")]` for TOML compatibility
- `.anim.toml` files go in `demo/animations/` directory next to the scene; loaded by scanning the directory at startup
- Skeletal animation uses separate `SkinnedVertex` type (6 attributes) and `skinned_pipeline` to avoid overhead on static geometry
- Bone matrices stored in storage buffer (bind group 3) — uniform buffers have size limits; storage supports arbitrary bone counts
- `SkeletalSync` bridges ECS to skeletal playback; `skeleton` component + `animator` component on same entity triggers skeletal path
- Crossfade blending via `blend_target`/`blend_duration` fields on `animator` component — uses pose array slerp/lerp

## Implemented vs Planned

**Working now:** Entity CRUD, scene load/save, query parsing/execution, schema introspection, PBR renderer with Cook-Torrance shading, glTF model import, cascaded shadow mapping, constraint validation + auto-fix, content-addressed asset catalog, egui GUI inspector, `serve --watch` hot-reload viewer, game loop with fixed-timestep physics, Rapier 3D character controller, first-person walkable scenes via `play` command, spatial audio with Kira (3D positioned sounds, ambient loops, event-driven triggers), property tween animation (Tier 1: keyframe clips with Step/Linear/CubicSpline, `.anim.toml` loading, `animator` component, event firing), skeletal animation (Tier 2: glTF skin/joint import, GPU vertex skinning via storage buffer, bone hierarchy computation, crossfade blending, skinned shadow mapping).

**Designed but not implemented:** Scripting (Rhai), AI asset generation pipeline. See `flint-design-doc.md` for full specification and `PHASE4_ROADMAP.md` for Phase 4 remaining stages (Stages 4-5).

## Project Structure

- `crates/` — All 16 workspace crates
- `schemas/` — Component and archetype TOML definitions (transform, door, bounds, material, rigidbody, collider, character_controller, player, audio_source, audio_listener, audio_trigger, animator, skeleton, etc.)
- `demo/` — Showcase scenes (showcase, phase3_showcase, phase4_runtime), demo scripts, `audio/` assets, and `animations/` clips
- `testGame/` — Test project directory (levels/, schemas/)
- `flint-design-doc.md` — Comprehensive design document covering all planned phases
- `PHASE4_ROADMAP.md` — Phase 4 status tracker (Stages 1-3 Tier 1 complete, Stages 3 Tier 2, 4-5 planned: Skeletal Animation, Scripting, Integration)
