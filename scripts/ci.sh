#!/usr/bin/env bash
# What a change must pass before it lands (#757).
#
#   scripts/ci.sh          formatting only: seconds, no build. `scripts/issue.sh
#                          land` runs this (verify= in scripts/issue.conf), and
#                          so does CI's rustfmt job.
#   scripts/ci.sh --full   also what CI's clippy and wasm jobs run: native
#                          `cargo clippy --all-targets` and scripts/wasm-check.sh.
#                          Minutes, and two builds.
#
# Formatting is rustfmt's defaults at edition 2021 (rustfmt.toml). Fix a failure
# with `cargo fmt --all`; one file alone is `rustfmt <file>`.
set -euo pipefail

cd "$(dirname "$0")/.."

echo "ci: cargo fmt --all --check"
if ! cargo fmt --all -- --check >/dev/null 2>&1; then
    echo "ci: these files are not formatted:" >&2
    cargo fmt --all -- --check -l 2>/dev/null | sort -u >&2 || true
    echo "ci: run \`cargo fmt --all\`, commit, and try again" >&2
    exit 1
fi

if [ "${1:-}" = "--full" ]; then
    echo "ci: cargo clippy --all-targets"
    cargo clippy --all-targets --keep-going
    echo "ci: scripts/wasm-check.sh"
    bash scripts/wasm-check.sh
fi

echo "ci: ok"
