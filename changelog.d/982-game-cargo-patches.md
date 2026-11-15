### A game binary's root `Cargo.toml` names the engine's `[patch.crates-io]`, `mach-dxcompiler-rs` included (#982)
**What changed.** cargo applies `[patch]` only from the root of the workspace it builds, so the static-DXC patch #947 added never reached a game with its own Rust binary: on Windows flint-render failed with five `E0425`s against crates.io's mach-dxcompiler-rs. flint-render now asks for exactly Flint's vendored crate (`=0.1.947`), which crates.io doesn't have. A game without the patch stops in cargo's resolver on every platform (``failed to select a version for the requirement `mach-dxcompiler-rs = "=0.1.947"` ``) and never downloads or links mach's prebuilt. `flint check <game>` warns `missing-cargo-patch` with the line to add.
**What to do.** In the game's workspace root `Cargo.toml`:
```toml
[patch.crates-io]
mach-dxcompiler-rs = { path = "engine/vendor/static-dxc" }
egui-winit = { path = "engine/vendor/egui-winit" }
```
If cargo then says the lockfile is `locked to 0.1.7+2026.10.26-141dd7f.1`, run `cargo update -p mach-dxcompiler-rs` once. A scenes-and-scripts game: nothing. Book: Upgrading the Engine, Cargo.
