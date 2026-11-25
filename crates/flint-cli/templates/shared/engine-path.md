The engine is the Flint checkout at `{{engine}}`. `flint.toml` and `bundle.toml` name its schemas by that path, which is right on this machine only. `flint build`, run in this project, builds a `flint` and a `flint-player` from that checkout with the packages `flint.toml` lists, and prints where they landed: use that `flint` from then on. `flint build --release` builds optimized ones.

**Taking an engine update.** Note the day of your last pull (`git -C {{engine}} log -1 --format=%cs`), pull that checkout and run `flint build` again. Then run `bash {{engine}}/scripts/changelog.sh since <that day>`: it prints every change a game can notice since then (scene and schema keys, script functions, command flags, changes in look or behaviour) and what to do about each. Then run `flint check` and play once, watching the log.

**A Rust binary of the game's own.** A crate of the game's that embeds the player (a player extension: `{{engine}}/docs/book/src/guides/player-extensions.md`) builds the engine from the game's own Cargo workspace, and cargo applies `[patch]` only from that workspace's root. So the game's root Cargo.toml names the engine's two patches itself, or cargo stops with `failed to select a version for the requirement mach-dxcompiler-rs`:

```toml
[patch.crates-io]
mach-dxcompiler-rs = { path = "{{engine}}/vendor/static-dxc" }
egui-winit = { path = "{{engine}}/vendor/egui-winit" }
```

`flint check` warns about a line the root lacks (`missing-cargo-patch`).

**Carrying the engine in the game.** Flint's games keep the engine inside their own repository as a git subtree, so a clone builds anywhere. To move this project to that pattern:

```bash
git init
git add .
git commit -m "Start {{name}}"
git remote add flint-engine https://github.com/chrischaps/Flint.git
git subtree add --prefix=engine flint-engine main --squash
```

Then set `schemas = ["engine/schemas", "schemas"]` in `flint.toml`, and delete the `schemas` line and the `[mount]` table from `bundle.toml`. Delete `scripts/lib/menu.rhai` too: it is a copy of the engine's, made because a script's `import` looks in the project and in engine/ but not in a checkout named by path, and once the subtree is there the same `import "menu"` finds the engine's own. From then on the engine's files are under engine/ in this project, and an update is `git subtree pull --prefix=engine flint-engine main --squash`.
