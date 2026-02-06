# Contributing

Flint is in active development (Phase 4 of 5). Contributions are welcome in these areas:

- **Bug reports** --- file issues on GitHub
- **Schema definitions** --- new component and archetype schemas
- **Documentation** --- improvements to this guide
- **Test coverage** --- additional unit and integration tests (146 tests across 16 crates)
- **Constraint kinds** --- new validation rule types
- **Physics** --- additional collider shapes, improved character controller behavior
- **Rendering** --- post-processing effects, LOD, additional debug views
- **Audio** --- additional audio formats, reverb zones, music system
- **Animation** --- blend trees, additive blending, animation state machines

## Development Setup

```bash
git clone https://github.com/chaps/flint.git
cd flint
cargo build
cargo test
cargo clippy
cargo fmt --check
```

## Running the Demo

```bash
# Scene viewer with hot-reload
cargo run --bin flint -- serve demo/phase4_runtime.scene.toml --watch

# First-person walkable scene
cargo run --bin flint -- play demo/phase4_runtime.scene.toml
```

## Code Style

- Run `cargo fmt` before committing
- Run `cargo clippy` and address warnings
- Each crate has its own error type using `thiserror`
- Tests live alongside the code they test (`#[cfg(test)]` modules)
- Prefer explicit over clever; readability over brevity

## Architecture

The project is a 16-crate Cargo workspace. See the [Architecture Overview](architecture/overview.md) and [Crate Dependency Graph](architecture/crate-graph.md) for how the crates relate to each other. Key principles:

- Dependencies flow in one direction (binary crates at the top, `flint-core` at the bottom)
- Components are dynamic `toml::Value`, not Rust types --- schemas are runtime data
- Two entry points: `flint-cli` (scene authoring) and `flint-player` (interactive gameplay)
