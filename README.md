# Flint

Flint is a 3D game engine written from scratch in Rust, and the foundation under a small family of games: a raft at sea, a clockwork platformer, a rhythm game, two racers, a Doom-style shooter and more. Its worlds are plain text. A scene is a TOML file that reads almost like a list (a sun this bright, a sea this rough, a raft right here), so a person, a small program or an AI assistant can read one, change it, and render it to a picture in seconds to check the result. That loop of write, render, look is the heart of the engine.

**Project page:** [chaps.dev/projects/flint](https://chaps.dev/projects/flint/) has a reel of every world that runs on Flint, a clip of one line of a scene file turning sunset into night, and a frame of FlintLeap peeled back layer by layer.

**Play it in the browser:** [play.chaps.dev](https://play.chaps.dev/) runs Flint games on WebAssembly and WebGPU, with no install.

![A moment in Starchild's clay world shown three ways: wireframe, surface directions and the finished image](https://chaps.dev/projects/flint/cover.jpg)

## Running it

You need a stable Rust toolchain. On Windows that is all; Android and the web need the extra targets described in the [documentation](https://docs.chaps.dev/flint).

```bash
cargo build --release
cargo run --bin flint -- play demo/phase4_runtime.scene.toml   # play a demo scene
cargo run --bin flint -- render demo/phase4_runtime.scene.toml --output shot.png --schemas schemas
cargo run --bin flint -- edit demo/phase4_runtime.scene.toml    # scene viewer with live inspector
cargo test
```

`flint init` starts a new game, which carries the engine inside it as a git subtree at `engine/` and layers its own schemas over the engine's (`--schemas engine/schemas --schemas schemas`). That is how every game in the family is built.

To run the web player locally (see `docs/design/web.md`):

```bash
rustup target add wasm32-unknown-unknown
cd crates/flint-player
trunk serve
```

## Layout

- `crates/` holds the 26-crate workspace: the `flint` CLI, `flint-player`, the wgpu renderer, Rapier physics, Kira audio, Rhai scripting, animation, particles, terrain, the query language and the editors
- `schemas/` defines the engine's components and archetypes in TOML
- `demo/` has showcase scenes, scripts and effects
- `docs/book/` is the mdBook source for [docs.chaps.dev/flint](https://docs.chaps.dev/flint)
- `tools/` holds the docs checker, the play.chaps.dev site builder and other helpers
