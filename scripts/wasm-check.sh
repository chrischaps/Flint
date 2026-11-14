#!/usr/bin/env bash
# Keep the web build honest (docs/design/web.md, epic #94).
#
# Builds flint-player and every workspace crate it links for
# wasm32-unknown-unknown, the way cargo check does, and denies the disallowed
# methods: the root clippy.toml's list (the vfs reads, #97; `as_float`, #685;
# the wgpu seams) plus scripts/wasm-clippy/wasm-only.toml's, the rules native
# clippy can't hold (Instant::now, SystemTime::now and their elapsed(), which
# panic on wasm32, #100; winit's cursor grab, Float#3). scripts/clippy-conf.sh
# puts the two together (#761). Every other clippy lint is off here; the
# native `cargo clippy` reads the root list alone.
#
#   scripts/wasm-check.sh        what CI's wasm job runs (.github/workflows/ci.yml)
#
# Runtime time calls use web_time (#100 ported the sites that predated
# this check); a std::time call there fails it.
set -euo pipefail

cd "$(dirname "$0")/.."
rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || true

conf="${CARGO_TARGET_DIR:-$PWD/target}/wasm-clippy"
bash scripts/clippy-conf.sh --wasm "$conf"

CLIPPY_CONF_DIR="$conf" \
    cargo clippy --target wasm32-unknown-unknown -p flint-player --bins --lib \
    -- -A clippy::all -D clippy::disallowed_methods
