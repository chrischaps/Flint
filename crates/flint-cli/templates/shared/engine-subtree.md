This project carries the engine as a git subtree at `engine`, and `flint.toml` names `engine/schemas`. `flint init` runs no git and no network commands, so the subtree is yours to add, once:

```bash
git init
git add .
git commit -m "Start {{name}}"
git remote add flint-engine https://github.com/chrischaps/Flint.git
git subtree add --prefix=engine flint-engine main --squash
```

Until it is there, every command stops with `schema directory not found`, naming `engine/schemas`. After it, build the engine the game carries:

```bash
cargo build --release --manifest-path engine/Cargo.toml
```

**Taking an engine update.** `git subtree pull --prefix=engine flint-engine main --squash`, then rebuild. The squash hides the engine's commit subjects, so read `engine/CHANGELOG.md` from the top down to the date of your last pull: it lists every change a game can notice (scene and schema keys, script functions, command flags, changes in look or behaviour) and what to do about each. Then run `flint validate` on your scenes and play once, watching the log.
