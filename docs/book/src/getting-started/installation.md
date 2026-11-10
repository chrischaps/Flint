# Installation

Flint is built from source using the Rust toolchain. There are no pre-built binaries yet.

## Prerequisites

- **Rust** (stable, 1.75+) --- install from [rustup.rs](https://rustup.rs/)
- **Git** --- for cloning the repository
- A GPU with **Vulkan**, **Metal**, or **DX12** support (for the renderer and viewer)

## Build from Source

Clone the repository and build in release mode:

```bash
git clone https://github.com/chrischaps/flint.git
cd flint
cargo build --release
```

The binary is at `target/release/flint` (or `target/release/flint.exe` on Windows).

On Linux the full build links the sound card (ALSA) and gamepads (libudev), so it needs their headers first:

```bash
sudo apt-get install -y pkg-config libasound2-dev libudev-dev
```

The editors' file dialogs go through the XDG desktop portal over D-Bus, which needs no headers to build, only a portal service at run time.

## Headless Build

A machine that only renders, generates or packs (a CI runner, a cloud agent, a build server) can leave the devices out:

```bash
cargo build -p flint-cli --no-default-features
```

Every command is still there. `flint render`, `gen`, `bundle`, `validate`, `render-suite`, `query` and the rest work exactly as in the full build, and their output is byte for byte the same, but nothing links ALSA or libudev, so a Linux box needs none of the packages above, and about sixty crates (the audio and gamepad backends and the dialogs' async D-Bus stack) aren't built at all. What leaves is each device:

| Feature | Off means |
|---------|-----------|
| `audio` | `flint play` runs silent, as on a machine with no sound card (sounds still load, scripts still play them). `calibrate` and `play-chart` won't start. `render-suite` renders offline either way. |
| `gamepad` | No pads are seen; `spike-rumble` says the build left them out. Keyboard, mouse, touch and `--input-script` still work. |
| `dialogs` | The editors' Save As, Export and Browse buttons open no dialog and say why on stderr. |

Put back any one of them with `--features`, for example `--no-default-features --features audio`. `bash scripts/headless-check.sh` checks that no device crate has crept back into the headless build's dependency tree, and CI builds it on a Linux runner with the ALSA and libudev headers removed.

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
