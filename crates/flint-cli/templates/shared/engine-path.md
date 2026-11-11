The engine is the Flint checkout at `{{engine}}`. `flint.toml` and `bundle.toml` name its schemas by that path, which is right on this machine only. The `flint` on your PATH should be built from the same checkout:

```bash
cargo build --release --manifest-path {{engine}}/Cargo.toml
```

**Taking an engine update.** Pull that checkout and rebuild. Then read `{{engine}}/CHANGELOG.md` from the top down to the date of your last pull: it lists every change a game can notice (scene and schema keys, script functions, command flags, changes in look or behaviour) and what to do about each. Then run `flint validate` on your scenes and play once, watching the log.

**Carrying the engine in the game.** Flint's games keep the engine inside their own repository as a git subtree, so a clone builds anywhere. To move this project to that pattern:

```bash
git init
git add .
git commit -m "Start {{name}}"
git remote add flint-engine https://github.com/chrischaps/Flint.git
git subtree add --prefix=engine flint-engine main --squash
```

Then set `schemas = ["engine/schemas", "schemas"]` in `flint.toml`, and delete the `schemas` line and the `[mount]` table from `bundle.toml`. From then on the engine's files are under engine/ in this project, and an update is `git subtree pull --prefix=engine flint-engine main --squash`.
