# Deploying to Android

Flint games can be packaged as Android APKs and run on physical devices. The engine uses the same wgpu/Vulkan rendering, Rhai scripting, and TOML scene format on mobile --- no code changes needed. Touch input, orthographic cameras, and 2D sprites work identically across desktop and Android.

## Prerequisites

### 1. Android SDK

Install via [Android Studio](https://developer.android.com/studio) or the standalone `sdkmanager`:

- **compileSdk**: API level 34
- **Build Tools**: 34.x
- **NDK**: latest version

### 2. Rust Android Target

```bash
rustup target add aarch64-linux-android
```

### 3. cargo-ndk

```bash
cargo install cargo-ndk
```

### 4. Environment Variables

```bash
export ANDROID_HOME=/path/to/android/sdk
export ANDROID_NDK_HOME=$ANDROID_HOME/ndk/<version>
```

## Building an APK

From the Flint engine root:

```bash
# Quick build + install to a connected device
./scripts/android-build.sh /path/to/your/game

# Or manual Gradle build
cd android
./gradlew assembleDebug -PgameDir=/path/to/your/game
```

The `-PgameDir` parameter points to your game project directory (the one containing scene files, schemas, scripts, etc.).

### What the Build Does

The Gradle build runs two tasks:

1. **`cargoNdkBuild`** --- Cross-compiles the `flint-android` crate as a native shared library (`libflint_android.so`) for ARM64 (`arm64-v8a`) using `cargo ndk`. The library is placed in `app/src/main/jniLibs/`.

2. **`copyGameAssets`** --- Copies your game's assets into the APK:
   - Scene files (`*.scene.toml`, `*.sprite.toml`, `*.anim.toml`)
   - Directories: `schemas/`, `scripts/`, `sprites/`, `textures/`, `models/`, `audio/`, `animations/`
   - Engine schemas are copied separately into `engine/schemas/`
   - Generates `asset_manifest.txt` listing every bundled file path

### Asset Manifest

Android's NDK `AAssetDir_getNextFileName()` only enumerates files within a directory --- it does not list subdirectories. To work around this, the Gradle build generates an `asset_manifest.txt` listing every relative file path. At runtime, the extractor reads this manifest and copies each file individually.

## How It Runs

When the APK launches:

1. **`android_main()`** initializes Android logging (visible in `logcat`)
2. **Asset extraction** copies all bundled files from the APK to internal storage so that `std::fs` code works unchanged --- no virtual filesystem needed
3. **Version marker** (`.asset_version`) prevents redundant extraction on subsequent launches
4. **Schema loading** loads engine schemas then game schemas (same merge order as desktop)
5. **Scene discovery** finds the first `*.scene.toml` file in the extracted assets
6. **Player event loop** starts the game using the same `PlayerApp` as desktop, with Android surface integration

## Architecture Decisions

### Extract, Don't Abstract

Rather than building a virtual filesystem that intercepts all file reads, Flint extracts APK assets to internal storage at startup. This means every `std::fs::read_to_string()`, `image::open()`, `gltf::import()`, and Rhai script load works exactly as on desktop. The extraction happens once, takes a fraction of a second for typical game assets.

### NativeActivity over GameActivity

Flint uses Android's built-in `NativeActivity` (`hasCode = "false"` in the manifest) rather than Google's Jetpack `GameActivity`. NativeActivity has zero Java dependencies --- the entire app is Rust.

### GPU Limits

Mobile GPUs may not support desktop-default wgpu limits. The Android entry point uses `wgpu::Limits::downlevel_defaults()` as a base, then overrides `max_texture_dimension_2d` with the adapter's actual reported capability.

### Platform API Level

The minimum API level is 26 (Android 8.0), required for:
- **AAudio** --- the audio backend used by Kira
- **Vulkan** --- the graphics backend used by wgpu

## Surface Lifecycle

Android can pause and resume apps at any time. Flint handles this gracefully:

- **`suspended()`** --- drops the window and surface, preserves GPU device and all game state
- **`resumed()`** --- recreates the window and surface, continues rendering

The first `resumed()` call performs full initialization (GPU device, pipelines, scene loading). Subsequent calls only recreate the surface.

## Game Project Structure

A typical game project targeting Android:

```
my_game/
  scenes/
    main.scene.toml       # Entry scene (first .scene.toml found)
  schemas/
    components/           # Game-specific components
    archetypes/           # Game-specific archetypes
  scripts/
    player.rhai
    hud.rhai
  sprites/
    player_sheet.png
  animations/
    character.sprite.toml
  audio/
    music.ogg
  input.toml              # Touch + keyboard bindings
```

The build copies this entire structure into the APK. Engine schemas are bundled separately, and the schema merge order (engine then game) is preserved.

## Testing on Desktop

Touch-zone bindings work on desktop via mouse emulation (enabled by default). Left-click-and-drag emulates a single finger touch, so you can test the full touch interaction model without a device.

```bash
# Test your touch-enabled game on desktop
flint play scenes/main.scene.toml --schemas schemas
```

## Debugging

### Logcat

All `log::info!`, `log::warn!`, and `log::error!` calls from Rust appear in Android logcat with the tag `flint`:

```bash
adb logcat -s flint:*
```

### Common Issues

| Issue | Cause | Fix |
|-------|-------|-----|
| Black screen on launch | Scene file not found in extracted assets | Check `asset_manifest.txt` includes your scene |
| Crash on surface creation | GPU doesn't support required features | Check `adb logcat` for wgpu errors; ensure Vulkan device |
| No audio | API level < 26 | AAudio requires Android 8.0+ |
| Touch not responding | `WindowEvent::Touch` not reaching input state | Verify `process_touch_*` calls in PlayerApp |

## Further Reading

- [Touch Input](../concepts/touch-input.md) --- touch zones, tap detection, and scripting API
- [2D Sprites](../concepts/sprites-2d.md) --- the rendering system for mobile-friendly 2D games
- [Building a Game Project](building-a-game-project.md) --- structuring a game that uses Flint as a subtree
