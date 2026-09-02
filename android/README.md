# Flint Android Build

Build and deploy Flint games to Android devices.

## Prerequisites

1. **Android SDK** — Install via Android Studio or `sdkmanager`
   - API level 34 (compileSdk)
   - Build Tools 34.x
   - NDK (latest)

2. **Rust Android targets**:
   ```bash
   rustup target add aarch64-linux-android
   ```

3. **cargo-ndk**:
   ```bash
   cargo install cargo-ndk
   ```

4. **Environment variables**:
   ```bash
   export ANDROID_HOME=/path/to/android/sdk
   export ANDROID_NDK_HOME=$ANDROID_HOME/ndk/<version>
   ```

## Building

From the engine root:

```bash
# Build debug APK for a game project
./scripts/android-build.sh /path/to/game/project

# Or manually:
cd android
./gradlew assembleDebug -PgameDir=/path/to/game/project
```

## How It Works

1. **`cargoNdkBuild`** — Runs `cargo ndk` to cross-compile `flint-android` (cdylib)
   for `arm64-v8a`. The resulting `.so` is placed in `app/src/main/jniLibs/`.

2. **`copyGameAssets`** — Copies game scene files, schemas, textures, scripts,
   models, and audio into `app/src/main/assets/` for APK bundling.

3. **At runtime** — `android_main()` extracts bundled assets from the APK to
   internal storage, then loads the scene and runs the player event loop.

## Game Assets

The `copyGameAssets` task copies these patterns from the game directory:
- `*.scene.toml`, `*.sprite.toml`, `*.anim.toml`
- `schemas/`, `scripts/`, `sprites/`, `textures/`, `models/`, `audio/`, `animations/`

Engine schemas are copied separately into `engine/schemas/` within assets.
