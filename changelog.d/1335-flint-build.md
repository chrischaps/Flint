### `flint build`, and `unbuilt-package` (#1335)
**What changed.** `flint build [--release] [--target <triple>] [--dry-run]
[--engine-path <dir>]` builds a project's `flint-player` and `flint` with
the packages its `[packages]` lists: each package's `[build] features` on
whichever of flint-player and flint-cli has them, plus each crate's build
options (`audio`, `gamepad`, `frame-history`, `dialogs`), as `cargo build
--no-default-features --features ...` in the engine workspace (the
project's `engine/`, else the engine its schemas come from). It prints the
cargo command and where the binaries landed; `--dry-run` builds nothing. A
project that drops `debug-hud` gets its player built in a cargo run of its
own, since flint-cli turns that feature on. On Windows a binary that is
running is renamed to `<name>.old.exe` first, and each binary deletes its
old copy when it next starts. With a `[packages]` table, `flint play`,
`flint-player` and `flint render` refuse to run when the binary lacks a
listed package (exit 2, naming `flint build`), and `flint check` reports it
as a new `unbuilt-package` error. The error for a command whose package a
build left out now says to list the package and run `flint build`.
**What to do.** Replace a `build.bat` line such as `cargo build --release
--bin flint-player --bin flint` with `flint build --release`. A game with no
`[packages]` table builds every package, as before. The first build, before
any `flint` exists, is `cargo run -p flint-cli -- build` from the engine
folder.
