#!/usr/bin/env bash
# Keep the web build honest (docs/design/web.md, epic #94).
#
# Builds flint-player and every workspace crate it links for
# wasm32-unknown-unknown, the way cargo check does, and denies the std calls
# that compile there but panic at run time (scripts/wasm-clippy/clippy.toml:
# Instant::now, SystemTime::now and their elapsed(), and the std::fs reads that
# must go through flint_core::vfs, #97). Every other clippy lint
# is off here; the native `cargo clippy` is untouched.
#
#   scripts/wasm-check.sh        what CI's wasm job runs (.github/workflows/ci.yml)
#
# A site that predates the check carries
#   #[allow(clippy::disallowed_methods)] // std time panics on wasm32; #100
# until #100 moves it to web-time. New code uses web_time instead.
set -euo pipefail

cd "$(dirname "$0")/.."
rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || true

CLIPPY_CONF_DIR="$PWD/scripts/wasm-clippy" \
    cargo clippy --target wasm32-unknown-unknown -p flint-player --bins --lib \
    -- -A clippy::all -D clippy::disallowed_methods
