### `cargo clippy` names a read past the vfs; the wasm list moved (#761)
**What changed.** The engine's root `clippy.toml` now lists the vfs reads
(`std::fs::read`, `read_to_string`, `read_dir`, `File::open`, `Path::exists`,
`is_file`, `is_dir`, `image::open`, `gltf::import`) and `toml::Value::as_float`
beside the wgpu seams, so native `cargo clippy` in the engine warns on them in
every crate the player links and fails in the five that draw. The wasm-only
rules (the std clock calls and winit's cursor grab) are
`scripts/wasm-clippy/wasm-only.toml`; `scripts/wasm-clippy/clippy.toml` is
gone, and `scripts/wasm-check.sh` builds its list from the two.
**What to do.** A game script or CI step that set
`CLIPPY_CONF_DIR=engine/scripts/wasm-clippy` should run
`engine/scripts/wasm-check.sh` instead, or point at the file
`bash engine/scripts/clippy-conf.sh --wasm <dir>` writes. A game's own crates
read their own `clippy.toml`; to hold the engine's rules, copy the root list's
`runtime` group into it. Otherwise nothing.
