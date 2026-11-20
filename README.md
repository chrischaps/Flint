# Flint

Flint is a 3D game engine written from scratch in Rust, and the foundation under a small family of games: a raft at sea, a clockwork platformer, a rhythm game, two racers, a Doom-style shooter and more. Its worlds are plain text. A scene is a TOML file that reads almost like a list (a sun this bright, a sea this rough, a raft right here), so a person, a small program or an AI assistant can read one, change it, and render it to a picture in seconds to check the result. That loop of write, render, look is the heart of the engine.

**Project page:** [chaps.dev/projects/flint](https://chaps.dev/projects/flint/) has a reel of every world that runs on Flint, a clip of one line of a scene file turning sunset into night, and a frame of FlintLeap peeled back layer by layer.

**Play it in the browser:** [play.chaps.dev](https://play.chaps.dev/) runs Flint games on WebAssembly and WebGPU, with no install.

**Demos:** [flint-demos](https://github.com/chrischaps/flint-demos) holds whole scenes made with Flint, short films that play themselves and places to walk through, with the engine carried as a subtree.

![A moment in Starchild's clay world shown three ways: wireframe, surface directions and the finished image](https://chaps.dev/projects/flint/cover.jpg)

## Running it

You need a stable Rust toolchain. On Windows that is all; Linux needs a few system packages, and Android and the web need extra targets, all described in the [documentation](https://docs.chaps.dev/flint).

From the Flint checkout:

```bash
cargo build --release
cargo run --release --bin flint -- play flint-examples/phase4_runtime.scene.toml   # play a demo scene
cargo run --release --bin flint -- render flint-examples/phase4_runtime.scene.toml --output shot.png
cargo run --release --bin flint -- edit flint-examples/phase4_runtime.scene.toml    # scene viewer with live inspector
cargo test
```

`--release` makes `cargo run` use the build above; without it Cargo compiles a second, debug build first.

### Putting `flint` on your PATH

From here on, and in what `flint init` prints, commands call `flint` by name. To make that work, install it from the checkout:

```bash
cargo install --locked --path crates/flint-cli
```

`--locked` builds with the dependency versions in `Cargo.lock`, the ones Flint is built and tested with. Without it, `cargo install` ignores the lock file and takes the newest versions allowed, and those can fail to compile (rhai 1.27, for one, breaks `flint-script`). It warns that a few locked packages are yanked and suggests running without `--locked`; ignore that. The command copies the binary into Cargo's bin folder (`~/.cargo/bin`, which rustup puts on your PATH). It is a copy, so run the command again after pulling engine changes, or `flint` stays the old version. To skip installing, call the build by its path: `target/release/flint` in the checkout (`flint.exe` on Windows).

### Starting a game

A game lives in its own folder, outside the Flint checkout. `flint init <name>` creates `<name>` under the directory you run it from, so step out of the checkout first:

```bash
cd ..                 # beside the Flint checkout, not inside it
flint init my-game
cd my-game
flint check
flint render scenes/main.scene.toml --output renders/main.png
flint play
```

Without installing, run `init` through Cargo from the checkout and give it a path outside, then call the build by path from the game's folder (here the checkout is `../Flint`):

```bash
cargo run --release --bin flint -- init ../my-game
cd ../my-game
../Flint/target/release/flint check
../Flint/target/release/flint render scenes/main.scene.toml --output renders/main.png
../Flint/target/release/flint play
```

`flint init` writes a `flint.toml` manifest, a lit scene with a player, a HUD script, an input config and a `CLAUDE.md` for agents, and prints the commands above. By default the manifest names the engine's schemas by the absolute path of the checkout that `flint` was built from, so those commands work at once, on this machine, as long as the checkout stays where it is. `flint init <name> --engine subtree` writes the pattern every game in the family uses, with the engine carried inside the game as a git subtree at `engine/` and the game's schemas layered over the engine's (`schemas = ["engine/schemas", "schemas"]` in its `flint.toml`), and prints the `git subtree add` to run.

To run the web player locally (see `docs/design/web.md`):

```bash
rustup target add wasm32-unknown-unknown
cd crates/flint-player
trunk serve
```

## Layout

- `crates/` holds the 33-crate workspace: the `flint` CLI, `flint-player`, the wgpu renderer, Rapier physics, Kira audio, Rhai scripting, animation, particles, terrain, the query language and the editors
- `schemas/` defines the engine's components and archetypes in TOML
- [flint-examples](https://github.com/chrischaps/flint-examples) has the showcase scenes, scripts and effects, in a repository of their own
- `docs/book/` is the mdBook source for [docs.chaps.dev/flint](https://docs.chaps.dev/flint)
- `tools/` holds the crate-graph analyzer and viewer, the play.chaps.dev site builder, the showcase film rig and other helpers (`tools/README.md` lists them); the docs checker is `scripts/docs-check.py`
