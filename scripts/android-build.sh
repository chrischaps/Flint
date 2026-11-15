#!/usr/bin/env bash
# Build a Flint game's APK and install it on a connected Android device.
# The one implementation: scripts\android-build.bat calls this through sh.cmd.
#
# Usage:
#   scripts/android-build.sh [game_dir]
#
# game_dir defaults to the engine's parent directory (the subtree layout, and
# what android/app/build.gradle.kts assumes without -PgameDir). Gradle builds
# the native library itself (its cargoNdkBuild task runs cargo ndk), copies the
# game's assets and packages the APK. With no android/gradlew, only the native
# library is built.

set -euo pipefail

usage() {
    cat <<'EOF'
Usage: android-build.sh [game_dir]

Builds the APK with Gradle (which runs cargo ndk for flint-android and copies
the game's assets) and installs it on a connected device with adb.

  game_dir    The game project (default: the directory above the engine).

Prerequisites:
  - Rust with the aarch64-linux-android target (rustup target add aarch64-linux-android)
  - cargo-ndk (cargo install cargo-ndk)
  - The Android NDK: ndk.dir in android/local.properties, or ANDROID_NDK_HOME
  - adb on PATH, for the install (without it the APK is built and left in place)
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
    usage
    exit 0
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ENGINE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
ANDROID_DIR="$ENGINE_DIR/android"
GAME_DIR="${1:-$(cd "$ENGINE_DIR/.." && pwd)}"

echo "=== Flint Android Build ==="
echo "Engine:  $ENGINE_DIR"
echo "Game:    $GAME_DIR"
echo ""

need() {
    if ! command -v "$1" &>/dev/null; then
        echo "[error] $1 is not on PATH. $2"
        exit 1
    fi
}
need cargo "Install Rust from https://rustup.rs/ and try again."
need cargo-ndk "Install it with: cargo install cargo-ndk"

if [[ ! -x "$ANDROID_DIR/gradlew" ]]; then
    echo "[1/1] No android/gradlew: building only the native library..."
    (cd "$ENGINE_DIR" && cargo ndk -t arm64-v8a -p flint-android --release)
    echo ""
    echo "Native library built. Set up the Gradle project in android/ to package an APK."
    exit 0
fi

echo "[1/2] Building APK (Gradle runs cargo ndk and copies the game's assets)..."
(cd "$ANDROID_DIR" && ./gradlew assembleDebug -PgameDir="$GAME_DIR")

APK="$ANDROID_DIR/app/build/outputs/apk/debug/app-debug.apk"
if [[ ! -f "$APK" ]]; then
    echo "[error] APK not found at $APK"
    exit 1
fi

if ! command -v adb &>/dev/null; then
    echo "[2/2] adb is not on PATH: skipping the install."
    echo "APK: $APK"
    exit 0
fi

echo "[2/2] Installing on device..."
adb install -r "$APK"

echo ""
echo "=== Done. Launch the app on your device. ==="
