### A listed package the launchers don't know yet: `flint check`, `flint play` and `flint render` name `package-sync.py` (#1589)
**Package.** engine
**What changed.** When `[packages]` lists a package whose folder is in the engine's `packages/` but whose `[build] features` neither launcher's `Cargo.toml` declares yet, `flint check` reports `unbuilt-package` and `flint play`, `flint-player`, `flint render` and a game's own binary refuse to run. Before, they passed over it without a word, because the binary had never heard of the feature. The hint names `python3 scripts/package-sync.py` in the engine, then `flint build`. Under `--json` the error document adds `unsynced`. A package the launchers already have still gets the plain `flint build` hint.
**What to do.** Nothing.
