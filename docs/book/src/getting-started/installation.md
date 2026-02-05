# Installation

Flint is built from source using the Rust toolchain. There are no pre-built binaries yet.

## Prerequisites

- **Rust** (stable, 1.75+) --- install from [rustup.rs](https://rustup.rs/)
- **Git** --- for cloning the repository
- A GPU with **Vulkan**, **Metal**, or **DX12** support (for the renderer and viewer)

## Build from Source

Clone the repository and build in release mode:

```bash
git clone https://github.com/chaps/flint.git
cd flint
cargo build --release
```

The binary is at `target/release/flint` (or `target/release/flint.exe` on Windows).

## Verify Installation

```bash
cargo run --bin flint -- --version
```

You should see the Flint version string.

## Running Without Installing

You can run Flint directly through Cargo without installing it system-wide:

```bash
cargo run --bin flint -- <command>
```

For example:

```bash
cargo run --bin flint -- init my-game
cargo run --bin flint -- serve demo/showcase.scene.toml --watch
```

## Running Tests

To verify everything is working:

```bash
cargo test
```

This runs the full test suite across all crates.

## Optional: Add to PATH

To use `flint` directly without `cargo run`:

```bash
cargo install --path crates/flint-cli
```

Or copy the release binary to a directory on your PATH.

## What's Next

With Flint built, follow [Your First Project](first-project.md) to create a scene from scratch.
