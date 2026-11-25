# {{name}}

The guide to this project for a coding agent is `CLAUDE.md`, in this folder: the edit, render, play loop, where each file lives, how to find components and script functions, and what trips people up. Read it before changing anything.

## Packages

The game has only the engine packages it lists under `[packages]` in `flint.toml`. `flint init` wrote that table at the end of the file, empty. The kernel is always in, and so are the standard packages `procgen` and `debug-hud` unless set to `false`. The feature packages `living`, `music`, `rhythm` and `ai` are in only when listed. `flint package list` shows each one and what it owns.

To add one, `living` for example, from this folder:

1. Add `living = "*"` on a line under `[packages]` in `flint.toml`. `"*"` takes any version. `flint package used --write` adds every package the project's scenes and scripts use.
2. Run `flint build`, with the `flint` you have. It runs cargo (so it needs Rust) to rebuild this game's `flint` and `flint-player` with the listed packages, and prints where the two landed: run those from then on. `flint build --dry-run` shows the plan and the cargo command first, and builds nothing. If the project has no `flint` yet, the engine section of `CLAUDE.md` has the first build.
3. Run `flint check` with the new `flint`. It is clean when the list and the binary agree. `unlisted-package` names a use of a package the list doesn't have, with the line to add, and `unbuilt-package` means the `flint` you ran was built without a listed package: run the one `flint build` printed.

`CLAUDE.md` has the rest, under "Packages".
