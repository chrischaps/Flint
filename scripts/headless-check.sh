#!/usr/bin/env bash
# Keep the headless CLI free of device crates (#221).
#
# `cargo build -p flint-cli --no-default-features` is flint with no sound
# card, gamepads or file dialogs: every command builds, and `render`, `gen`,
# `bundle`, `validate` and the rest work the same, without ALSA or libudev
# headers on a Linux machine. A default dependency added to a crate the
# CLI links can quietly bring one back, so this:
#
#   1. reads the headless dependency tree for Linux (from any host) and fails
#      if a crate that links a device library is in it, naming who pulls it;
#   2. with --build, checks flint-cli headless for this machine, which on a
#      Linux box with none of those headers is the real proof (CI's job).
#
#   scripts/headless-check.sh           the tree only (seconds, any host)
#   scripts/headless-check.sh --build   the tree, then cargo check
set -euo pipefail

cd "$(dirname "$0")/.."

# Crates that open a device or link a desktop toolkit at build time.
FORBIDDEN='^(alsa-sys|alsa|cpal|libudev-sys|libudev|gilrs|gilrs-core|rfd|gtk-sys|gtk|gdk-sys|glib-sys|gobject-sys|atk-sys|pango-sys|cairo-sys-rs|rusty-xinput) v'

tree=$(cargo tree -p flint-cli --no-default-features -e normal,build \
    --target x86_64-unknown-linux-gnu --prefix none)
found=$(printf '%s\n' "$tree" | grep -E "$FORBIDDEN" | sort -u || true)
if [ -n "$found" ]; then
    echo "headless flint-cli links device crates on Linux:" >&2
    printf '%s\n' "$found" | sed 's/^/  /' >&2
    first=$(printf '%s\n' "$found" | head -n1 | cut -d' ' -f1)
    echo "who pulls $first:" >&2
    cargo tree -p flint-cli --no-default-features -e normal,build,features \
        --target x86_64-unknown-linux-gnu -i "$first" >&2 || true
    exit 1
fi
echo "headless flint-cli tree for Linux: no device crates" \
    "($(printf '%s\n' "$tree" | sort -u | wc -l) crates)"

if [ "${1:-}" = "--build" ]; then
    cargo check -p flint-cli --no-default-features --bins
fi
