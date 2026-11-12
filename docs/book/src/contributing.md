# Contributing

The five numbered engine phases (ECS, constraints, PBR rendering, runtime, AI assets) are long complete. Since then the engine has grown by feature programmes tracked in numbered Architecture Decision Records rather than phases, each a file in [`docs/adr/`](https://github.com/chrischaps/Flint/tree/main/docs/adr) (the early records were written in the Starchild game repo, which is gone; they were rebuilt from the commits that cite them, #374): the ocean and sky pipelines, render modes, the music-session runtime, the clay-look shading levers, the consolidated F4 render menu, animation layers and sequences, and the player-app decomposition. Contributions are welcome in these areas:

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
- **Procedural generators** --- new `flint-procgen` generators; the crate's [CONTRIBUTING guide](https://github.com/chrischaps/Flint/blob/main/crates/flint-procgen/CONTRIBUTING.md) builds one from scratch, schema and tests included

## Development Setup

```bash
git clone https://github.com/chrischaps/flint.git
cd flint
cargo build
cargo test
cargo clippy
cargo fmt --all
bash scripts/ci.sh        # what a PR must pass: formatting (--full adds clippy and the wasm check)
```

## Running the Examples

`examples/` holds the curated scenes, each listed in `examples/index.toml` with a topic and a one-line purpose:

```bash
cargo run --bin flint -- examples                  # every example, by topic
cargo run --bin flint -- examples --topic stylize   # one topic
cargo run --bin flint -- examples --check           # what CI runs: indexed and valid
```

A new example needs an entry in the index. The examples are free to change: the render goldens load frozen copies under `tests/fixtures/scenes/`, never `examples/`, so improving one never means re-blessing a golden. A scene only a test needs goes in `tests/fixtures/scenes/`, with the assets it loads.

```bash
# Scene viewer with hot-reload
cargo run --bin flint -- edit examples/phase4_runtime.scene.toml --watch

# First-person walkable scene
cargo run --bin flint -- play examples/phase4_runtime.scene.toml

# Headless snapshot (the primary validation tool for agents)
cargo run --release --bin flint -- render examples/showcase_tavern.scene.toml -o tavern.png
```

Release builds are strongly recommended for `render`, `play`, and the full test suite; debug builds of `flint-cli` need the 8 MB main-thread stack that `.cargo/config.toml` links on Windows.

## Code Style

- Run `cargo fmt --all` before committing. CI's `rustfmt` job and `scripts/issue.sh land` both refuse an unformatted file
- Run `cargo clippy` and address warnings
- Each crate has its own error type using `thiserror`
- Tests live alongside the code they test (`#[cfg(test)]` modules)
- Prefer explicit over clever; readability over brevity

## The Changelog

Games carry the engine as a squashed subtree, so the changelog is how they learn what an update changed. A change a game can notice (a scene or schema key, a script function, a CLI flag, a file format, a Cargo feature, a change in look or behaviour) gets an entry, and each pull request writes its own file:

```bash
bash scripts/changelog.sh new 123 look-takes-a-path   # writes changelog.d/123-look-takes-a-path.md
```

Fill it in: a `### <title> (#123)` line naming the key, function or flag as a game author types it, then **What changed.** and **What to do.**, about 150 words in all. Two PRs never write the same file, so entries never conflict. After the merge, the `Changelog` workflow moves the entry into `changelog/YYYY-MM.md` under the merge's date and deletes the fragment; `CHANGELOG.md` is a short page that links the months.

CI's `changelog` job fails a PR that touches `schemas/`, the script API or the scene format with no fragment. If no game can notice the change, a commit-message line waives it:

```
Changelog: none (reason)
```

`bash scripts/changelog.sh since <YYYY-MM-DD>` prints every entry from that day on, fragments included; `bash scripts/changelog-test.sh` tests the scripts.

## Architecture

The project is a 27-member Cargo workspace: 26 crates under `crates/` plus the `tools/arch-analyzer` static analyzer. See the [Architecture Overview](architecture/overview.md) and [Crate Dependency Graph](architecture/crate-graph.md) for how the crates relate to each other. Key principles:

- Dependencies flow in one direction (binary crates at the top, `flint-core` at the bottom)
- Components are dynamic `toml::Value`, not Rust types --- schemas are runtime data
- Two entry points: `flint-cli` (scene authoring and rhythm tooling) and `flint-player` (interactive gameplay), with the CLI embedding the player
- Debug surfaces (F3 scene panels, F4 Rendering & Effects) live in `flint-debug-ui` behind the player's default-on `debug-hud` feature; feature-off builds must still compile
