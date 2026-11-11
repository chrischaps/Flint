# Installation

Flint is built from source using the Rust toolchain. There are no pre-built binaries yet.

## Prerequisites

- **Rust** (stable, 1.75+) --- install from [rustup.rs](https://rustup.rs/)
- **Git** --- for cloning the repository
- A GPU with **Vulkan**, **Metal**, or **DX12** support (for the renderer and viewer), or a software adapter for headless renders ([below](#to-render-without-a-gpu))
- On Linux, a few system packages ([Linux and Headless Setup](#linux-and-headless-setup))

## Build from Source

Clone the repository and build in release mode:

```bash
git clone https://github.com/chrischaps/flint.git
cd flint
cargo build --release
```

The binary is at `target/release/flint` (or `target/release/flint.exe` on Windows).

## Linux and Headless Setup

On Linux, a fresh clone needs a few system packages before `cargo build` gets through, and a machine with no GPU (a container, a CI runner, a cloud agent) needs one more before `flint render` can draw. This list was checked one package at a time on a stock Ubuntu 24.04 image with no GPU and no `/dev/dri` (#220).

### To build

```bash
sudo apt-get install build-essential pkg-config libasound2-dev libudev-dev
```

| Package | Needed by | Without it |
|---------|-----------|------------|
| `build-essential` | `cc`, for crates with C parts | no linker or C compiler |
| `pkg-config` | the `-sys` build scripts below, to find their libraries | both fail |
| `libasound2-dev` | `alsa-sys` (audio: kira → cpal → alsa) | `failed to run custom build command for alsa-sys` |
| `libudev-dev` | `libudev-sys` (gamepads: gilrs) | `failed to run custom build command for libudev-sys` |

With those four, `cargo build -p flint-cli` finishes. A prebuilt `flint` on another machine needs only the runtime halves: `libasound2t64` (`libasound2` before Ubuntu 24.04) and `libudev1`, which base images already have. Without the ALSA library the binary doesn't start: `error while loading shared libraries: libasound.so.2`.

The editors' file dialogs go through the XDG desktop portal over D-Bus, which needs no headers to build, only a portal service at run time.

Other distributions need the same things under their own names (a C toolchain, pkg-config, the ALSA and libudev development files, Mesa's Vulkan drivers). Only the Ubuntu list above has been tested.

### To build without the devices

A machine that only renders, generates or packs (a CI runner, a cloud agent, a build server) can leave the devices out:

```bash
cargo build -p flint-cli --no-default-features
```

Every command is still there. `flint render`, `gen`, `bundle`, `validate`, `render-suite`, `query` and the rest work exactly as in the full build, and their output is byte for byte the same, but nothing links ALSA or libudev, so a Linux box needs only `build-essential` and `pkg-config` from the list above, and about sixty crates (the audio and gamepad backends and the dialogs' async D-Bus stack) aren't built at all. What leaves is each device:

| Feature | Off means |
|---------|-----------|
| `audio` | `flint play` runs silent, as on a machine with no sound card (sounds still load, scripts still play them). `calibrate` and `play-chart` won't start. `render-suite` renders offline either way. |
| `gamepad` | No pads are seen; `spike-rumble` says the build left them out. Keyboard, mouse, touch and `--input-script` still work. |
| `dialogs` | The editors' Save As, Export and Browse buttons open no dialog and say why on stderr. |

Put back any one of them with `--features`, for example `--no-default-features --features audio`. `bash scripts/headless-check.sh` checks that no device crate has crept back into the headless build's dependency tree, and CI builds it on a Linux runner with the ALSA and libudev headers removed.

### To render without a GPU

wgpu needs a Vulkan driver, and a machine with no GPU has none. Mesa's lavapipe is a Vulkan driver that runs on the CPU:

```bash
sudo apt-get install mesa-vulkan-drivers   # also pulls in libvulkan1, the Vulkan loader
flint render --list-gpus
#   llvmpipe (LLVM 20.1.2, 256 bits) [Cpu, Vulkan]
```

Without it, `--list-gpus` lists nothing and `flint render` stops, and both say what to install:

```text
Error: Failed to create headless render context

Caused by:
    Failed to get adapter: with no GPU, install a software Vulkan driver (lavapipe): `sudo apt-get install mesa-vulkan-drivers` on Debian/Ubuntu, then `flint render --list-gpus` should list "llvmpipe"
```

Lavapipe is slow but works: a debug build drew `examples/pbr_test.scene.toml` at 960x540 in about 14 s in that container, and #220's three ~22k-vertex trees took about 6 s. Debug builds also log `InstanceFlags::VALIDATION requested, but unable to find layer: VK_LAYER_KHRONOS_validation`; that warning is harmless. Lavapipe's renders are byte-identical from run to run. Pass `--no-grid`, though: lavapipe draws the editor grid with stray triangles across the near ground, where a GPU draws clean lines. Without the grid, that frame matched an RTX's in all but 0.99% of its pixels, and only 4 of those were off by more than 8 levels. Goldens still want a per-adapter variant (`.lavapipe.png`).

On Windows the software adapter is WARP, which every Windows ships: `WGPU_BACKEND=dx12 WGPU_ADAPTER_NAME="Microsoft Basic Render Driver"`. wgpu only offers it on a machine with no GPU, which is how CI runs the GPU tests and render goldens ([Golden Suites](../guides/headless-rendering.md#golden-suites)).

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
cargo run --bin flint -- serve examples/showcase.scene.toml --watch
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
