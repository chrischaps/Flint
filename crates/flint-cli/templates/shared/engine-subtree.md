This project carries the engine as a git subtree at `engine`, and `flint.toml` names `engine/schemas`. `flint init` runs no git and no network commands, so the subtree is yours to add, once:

```bash
git init
git add .
git commit -m "Start {{name}}"
git remote add flint-engine https://github.com/chrischaps/Flint.git
git subtree add --prefix=engine flint-engine main --squash
```

Until it is there, every command stops with `schema directory not found`, naming `engine/schemas`. After it, build the game's `flint` and `flint-player` from the engine it carries, with the packages `flint.toml` lists. The first time, run the engine's own `flint build` through cargo, from this folder:

```bash
cargo run --manifest-path engine/Cargo.toml -p flint-cli -- build --release
```

It prints where the two binaries landed, in the engine's release folder: use that `flint` from then on, and after a change to `[packages]` run `flint build` again.

**Taking an engine update.** Note the day of your last pull (`git log -1 --format=%cs -- engine`), then `git subtree pull --prefix=engine flint-engine main --squash` and run `flint build` again. The squash hides the engine's commit subjects, so run `bash engine/scripts/changelog.sh since <that day>`: it prints every change a game can notice since then (scene and schema keys, script functions, command flags, changes in look or behaviour) and what to do about each. Then run `flint check` and play once, watching the log.

**A Rust binary of the game's own.** A crate of the game's that embeds the player (a player extension: `engine/docs/book/src/guides/player-extensions.md`) builds the engine from the game's own Cargo workspace, and cargo applies `[patch]` only from that workspace's root. So the game's root Cargo.toml names the engine's two patches itself, or cargo stops with `failed to select a version for the requirement mach-dxcompiler-rs`:

```toml
[workspace]
exclude = ["engine"]

[patch.crates-io]
mach-dxcompiler-rs = { path = "engine/vendor/static-dxc" }
egui-winit = { path = "engine/vendor/egui-winit" }
```

`flint check` warns about a line the root lacks (`missing-cargo-patch`). On Windows the binary also asks for its own 8 MB main-thread stack, as the engine's `flint` and `flint-player` do in their build scripts: the guide has the build script, a dozen lines.
