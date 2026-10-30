# Contributing

The five numbered engine phases (ECS, constraints, PBR rendering, runtime, AI assets) are long complete. Since then the engine has grown by feature programmes tracked in numbered Architecture Decision Records (ADRs 0001-0067, kept in the Starchild game repo) rather than phases: the ocean and sky pipelines, render modes, the music-session runtime, the clay-look shading levers, the consolidated F4 render menu, animation layers and sequences, and the player-app decomposition. Contributions are welcome in these areas:

- **Bug reports** --- file issues on GitHub
- **Schema definitions** --- new component and archetype schemas
- **Documentation** --- improvements to this guide
- **Test coverage** --- additional unit and integration tests (1,107 tests across the 24 default workspace crates; 6 ignored)
- **Constraint kinds** --- new validation rule types
- **Physics** --- additional collider shapes, improved character controller behavior
- **Rendering** --- post-processing effects, LOD, additional debug views
- **Audio** --- additional audio formats, reverb zones
- **Music sessions** --- chart authoring tools, more verb maps, ladder and seam tuning
- **Animation** --- blend trees, animation state machines
- **Scripting** --- new Rhai API functions, script debugging tools, performance profiling
- **AI generation** --- new provider integrations, improved style validation, prompt engineering

## Development Setup

```bash
git clone https://github.com/chrischaps/flint.git
cd flint
cargo build
cargo test
cargo clippy
cargo fmt --check
```

## Running the Demo

```bash
# Scene viewer with hot-reload
cargo run --bin flint -- edit demo/phase4_runtime.scene.toml --watch

# First-person walkable scene
cargo run --bin flint -- play demo/phase4_runtime.scene.toml

# Headless snapshot (the primary validation tool for agents)
cargo run --release --bin flint -- render demo/showcase_tavern.scene.toml -o tavern.png --schemas schemas
```

Release builds are strongly recommended for `render`, `play`, and the full test suite; debug builds of `flint-cli` need the 8 MB main-thread stack that `.cargo/config.toml` links on Windows.

## Code Style

- Run `cargo fmt` before committing
- Run `cargo clippy` and address warnings
- Each crate has its own error type using `thiserror`
- Tests live alongside the code they test (`#[cfg(test)]` modules)
- Prefer explicit over clever; readability over brevity

## Architecture

The project is a 27-member Cargo workspace: 26 crates under `crates/` plus the `tools/arch-analyzer` static analyzer. See the [Architecture Overview](architecture/overview.md) and [Crate Dependency Graph](architecture/crate-graph.md) for how the crates relate to each other. Key principles:

- Dependencies flow in one direction (binary crates at the top, `flint-core` at the bottom)
- Components are dynamic `toml::Value`, not Rust types --- schemas are runtime data
- Two entry points: `flint-cli` (scene authoring and rhythm tooling) and `flint-player` (interactive gameplay), with the CLI embedding the player
- Debug surfaces (F3 scene panels, F4 Rendering & Effects) live in `flint-debug-ui` behind the player's default-on `debug-hud` feature; feature-off builds must still compile
