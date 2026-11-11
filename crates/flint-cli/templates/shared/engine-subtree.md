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

**Taking an engine update.** Note the day of your last pull (`git log -1 --format=%cs -- engine`), then `git subtree pull --prefix=engine flint-engine main --squash` and rebuild. The squash hides the engine's commit subjects, so run `bash engine/scripts/changelog.sh since <that day>`: it prints every change a game can notice since then (scene and schema keys, script functions, command flags, changes in look or behaviour) and what to do about each. Then run `flint validate` on your scenes and play once, watching the log.
