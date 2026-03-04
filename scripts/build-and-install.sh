#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR/.."

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
    echo "Usage: build-and-install.sh"
    echo ""
    echo "Builds Flint in release mode and installs:"
    echo "  - flint"
    echo "  - flint-player"
    echo ""
    echo "Binaries are installed to ~/.cargo/bin."
    exit 0
fi

if ! command -v cargo &>/dev/null; then
    echo "[error] Cargo is not available on PATH."
    echo "Install Rust from https://rustup.rs/ and try again."
    exit 1
fi

echo "[1/3] Building workspace in release mode..."
cargo build --release --locked

echo "[2/3] Installing flint CLI..."
cargo install --path crates/flint-cli --force --locked

echo "[3/3] Installing flint player..."
cargo install --path crates/flint-player --force --locked

echo ""
echo "Build and install completed successfully."
echo "Installed binaries are in ~/.cargo/bin"
