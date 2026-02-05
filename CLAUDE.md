# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Flint is a CLI-first, AI-agent-optimized 3D game engine written in Rust. The core thesis inverts traditional engine design: the primary interface is CLI and code, with visual tools focused on *validating* results rather than *creating* them. Currently in Phase 1 (Foundation) with basic rendering from Phase 3 also implemented.

## Build & Development Commands

```bash
cargo build                    # Debug build
cargo build --release          # Release build
cargo run --bin flint -- <cmd> # Run CLI (e.g., cargo run --bin flint -- serve demo/showcase.scene.toml --watch)
cargo test                     # Run all tests (34 unit tests across crates)
cargo test -p flint-core       # Test a single crate
cargo clippy                   # Lint
cargo fmt --check              # Check formatting
```

## Architecture

Seven-crate Cargo workspace with clear dependency layering:

```
flint-cli          CLI binary entry point (clap derive). Commands: init, entity, scene, query, schema, serve
  ├── flint-query   PEG query language (pest parser). Syntax: "entities where archetype == 'door'"
  ├── flint-scene   TOML scene serialization/deserialization with load/save
  ├── flint-render  wgpu-based 3D renderer (winit 0.30 ApplicationHandler pattern, wgpu 23)
  ├── flint-ecs     hecs wrapper with stable EntityId mapping (BiMap), named entities, parent/child
  ├── flint-schema  Component/archetype schema system with TOML loading and runtime validation
  └── flint-core    Fundamental types: EntityId, ContentHash, Transform, Vec3, Color, FlintError
```

## Key Design Decisions

- **Dynamic components** stored as `toml::Value` (not Rust types) so archetypes are defined at runtime in schema TOML files
- **Stable EntityIds** via atomic counter (never recycled), persisted across save/load with counter adjustment to prevent collisions
- **Scene format is TOML** — human-readable, diffable, structured as `[scene]` metadata + `[entities.<name>]` sections
- **Schemas live in `schemas/`** directory — `components/*.toml` define field types and constraints, `archetypes/*.toml` bundle components with defaults
- **Query language** uses pest PEG parser (`grammar.pest`). Supports: `==`, `!=`, `contains`, `>`, `<`, `>=`, `<=` on entity fields
- **Scene reload** is full re-parse (not incremental) to avoid borrow checker issues with `Mutex<ViewerState>`

## Technical Gotchas

- `toml::toml!` macro produces `Map<String, Value>` not `Value` — needs `.into()` in tests
- wgpu v23 `Instance::new()` takes owned `InstanceDescriptor`, not a reference
- winit v0.30 uses `ApplicationHandler` trait with `run_app()` — no direct event loop matching
- `FlintError` uses `thiserror`; crate-level `Result<T>` aliases throughout

## Implemented vs Planned

**Working now:** Entity CRUD, scene load/save, query parsing/execution, schema introspection, basic wgpu renderer with archetype-based coloring, `serve --watch` hot-reload viewer with orbit camera.

**Designed but not implemented:** Constraint system (validation + auto-fix), asset system (content-addressed), physics (Rapier), audio (Kira), scripting (Rhai), viewer GUI, AI asset generation pipeline. See `flint-design-doc.md` for full specification.

## Project Structure

- `crates/` — All seven workspace crates
- `schemas/` — Example component and archetype TOML definitions
- `demo/` — Showcase scene and PowerShell demo scripts
- `testGame/` — Test project directory (levels/, schemas/)
- `flint-design-doc.md` — Comprehensive 760-line design document covering all planned phases
