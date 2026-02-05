# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Flint is a CLI-first, AI-agent-optimized 3D game engine written in Rust. The core thesis inverts traditional engine design: the primary interface is CLI and code, with visual tools focused on *validating* results rather than *creating* them. Currently through Phase 4 Stage 1 (Game Loop + Physics) with PBR rendering from Phase 3, constraints from Phase 2, and interactive first-person gameplay.

## Build & Development Commands

```bash
cargo build                    # Debug build
cargo build --release          # Release build
cargo run --bin flint -- <cmd> # Run CLI (e.g., cargo run --bin flint -- serve demo/showcase.scene.toml --watch)
cargo run --bin flint -- play demo/phase4_runtime.scene.toml  # Play a scene with first-person controls
cargo run --bin flint-player -- demo/phase4_runtime.scene.toml --schemas schemas  # Standalone player
cargo test                     # Run all tests (107 unit tests across crates)
cargo test -p flint-core       # Test a single crate
cargo clippy                   # Lint
cargo fmt --check              # Check formatting
```

## Architecture

14-crate Cargo workspace with clear dependency layering:

```
flint-cli           CLI binary (clap derive). Commands: init, entity, scene, query, schema, serve, play, validate, asset, render
flint-player        Standalone player binary with game loop, physics, first-person controls
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

## Technical Gotchas

- `toml::toml!` macro produces `Map<String, Value>` not `Value` — needs `.into()` in tests
- wgpu v23 `Instance::new()` takes owned `InstanceDescriptor`, not a reference
- winit v0.30 uses `ApplicationHandler` trait with `run_app()` — no direct event loop matching
- `FlintError` uses `thiserror`; crate-level `Result<T>` aliases throughout
- Rapier v0.22 character controller types are in `rapier3d::control`, not `rapier3d::prelude`
- `DeviceEvent::MouseMotion` provides raw mouse delta for first-person camera (independent of cursor position)
- Rapier's `ChannelEventCollector` requires `crossbeam` channels for collision events

## Implemented vs Planned

**Working now:** Entity CRUD, scene load/save, query parsing/execution, schema introspection, PBR renderer with Cook-Torrance shading, glTF model import, cascaded shadow mapping, constraint validation + auto-fix, content-addressed asset catalog, egui GUI inspector, `serve --watch` hot-reload viewer, game loop with fixed-timestep physics, Rapier 3D character controller, first-person walkable scenes via `play` command.

**Designed but not implemented:** Audio (Kira), scripting (Rhai), AI asset generation pipeline. See `flint-design-doc.md` for full specification and `PHASE4_ROADMAP.md` for Phase 4 remaining stages.

## Project Structure

- `crates/` — All 14 workspace crates
- `schemas/` — Component and archetype TOML definitions (transform, door, bounds, material, rigidbody, collider, character_controller, player, etc.)
- `demo/` — Showcase scenes (showcase, phase3_showcase, phase4_runtime) and demo scripts
- `testGame/` — Test project directory (levels/, schemas/)
- `flint-design-doc.md` — Comprehensive design document covering all planned phases
- `PHASE4_ROADMAP.md` — Phase 4 status tracker (Stage 1 complete, Stages 2-4 planned)
