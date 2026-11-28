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
# It holds the smallest player too (#754): `flint-player
# --no-default-features` links none of the optional parts' crates (the
# music, the rhythm game, procgen, living props and the wooze field, the
# debug panels, the terrain, #1623), and with --build it compiles, tests and
# examples included.
# And flint-render links no flint-wooze (#1330), flint-terrain or flint-particles
# (#1615). And the headless CLI is the
# one with no package either (#1333): none of the music, rhythm, living and
# AI crates, and no flint-wooze, whose readers in flint-procgen are behind its
# `living` feature (#1516). Nor does the viewer with no features.
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

# The packages' crates (packages/<name>/package.toml) a flint-cli with no
# package feature leaves out
CLI_OPTIONAL='^(flint-music|flint-rhythm|flint-input-capture|flint-rhythm-game|flint-living|flint-wooze|flint-procgen|flint-procgen-cli|flint-asset-gen|flint-procgen-ai|flint-ai-core|flint-debug-ui|flint-terrain) v'
found=$(printf '%s\n' "$tree" | grep -E "$CLI_OPTIONAL" | sort -u || true)
if [ -n "$found" ]; then
    echo "flint-cli with no default features links a package's crate:" >&2
    printf '%s\n' "$found" | sed 's/^/  /' >&2
    first=$(printf '%s\n' "$found" | head -n1 | cut -d' ' -f1)
    cargo tree -p flint-cli --no-default-features -e normal,features -i "$first" >&2 || true
    exit 1
fi
echo "smallest flint-cli: none of music, rhythm, living, procgen, ai, wooze, debug-hud, terrain"

# The viewer's [living] and [wooze] are the living package's, through a point
# its CLI part registers (#1558), and its widgets and F4 are the kernel's
# flint-widgets (#1583), and a scene's terrain is the terrain package's scene
# binder, handed over through the points (#1623): the viewer links none of
# flint-procgen, flint-wooze, flint-debug-ui and flint-terrain in any build
viewer=$(cargo tree -p flint-viewer --no-default-features -e normal --prefix none)
found=$(printf '%s\n' "$viewer" | grep -E '^(flint-wooze|flint-procgen|flint-debug-ui|flint-terrain) v' | sort -u || true)
if [ -n "$found" ]; then
    echo "flint-viewer links a package's crate:" >&2
    printf '%s\n' "$found" | sed 's/^/  /' >&2
    first=$(printf '%s\n' "$found" | head -n1 | cut -d' ' -f1)
    cargo tree -p flint-viewer --no-default-features -e normal,features -i "$first" >&2 || true
    exit 1
fi
echo "smallest flint-viewer: no flint-wooze, no flint-procgen, no flint-debug-ui, no flint-terrain"

OPTIONAL='^(flint-music|flint-rhythm|flint-input-capture|flint-rhythm-game|flint-procgen|flint-living|flint-wooze|flint-debug-ui|flint-terrain) v'
player=$(cargo tree -p flint-player --no-default-features -e normal --prefix none)
found=$(printf '%s\n' "$player" | grep -E "$OPTIONAL" | sort -u || true)
if [ -n "$found" ]; then
    echo "flint-player with no default features links an optional part's crate:" >&2
    printf '%s\n' "$found" | sed 's/^/  /' >&2
    exit 1
fi
echo "smallest flint-player: none of music, rhythm, procgen, living, wooze, debug-hud, terrain"

# The renderer takes the wooze field's answers as flint_core::field_frame
# data and links nothing of the field (#1330)
if cargo tree -p flint-render -e normal --prefix none | grep -qE '^flint-wooze v'; then
    echo "flint-render links flint-wooze:" >&2
    cargo tree -p flint-render -e normal -i flint-wooze >&2 || true
    exit 1
fi
echo "flint-render: no flint-wooze"

# Nor the terrain's or the particles' crates (#1615, ADR 0138 decision 5):
# it draws the plain data they hand it (`flint_render::particles`,
# flint-geometry's `TerrainChunk` and `GrassConfig`)
render=$(cargo tree -p flint-render -e normal --prefix none)
found=$(printf '%s\n' "$render" | grep -E '^(flint-terrain|flint-particles) v' | sort -u || true)
if [ -n "$found" ]; then
    echo "flint-render links a package's crate:" >&2
    printf '%s\n' "$found" | sed 's/^/  /' >&2
    first=$(printf '%s\n' "$found" | head -n1 | cut -d' ' -f1)
    cargo tree -p flint-render -e normal -i "$first" >&2 || true
    exit 1
fi
echo "flint-render: no flint-terrain, no flint-particles"

if [ "${1:-}" = "--build" ]; then
    cargo check -p flint-cli --no-default-features --bins
    cargo check -p flint-player --no-default-features --all-targets
fi
