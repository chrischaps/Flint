#!/bin/bash
# Build and install a Flint game APK on a connected Android device.
#
# Usage:
#   ./scripts/android-build.sh [game_dir]
#
# game_dir defaults to the engine's parent directory (typical subtree layout).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ENGINE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
ANDROID_DIR="$ENGINE_DIR/android"

GAME_DIR="${1:-$(cd "$ENGINE_DIR/.." && pwd)}"

echo "=== Flint Android Build ==="
echo "Engine:  $ENGINE_DIR"
echo "Game:    $GAME_DIR"
echo ""

cd "$ANDROID_DIR"

# Build the APK (Gradle will invoke cargo-ndk and copy assets)
echo "[1/2] Building APK..."
./gradlew assembleDebug -PgameDir="$GAME_DIR"

APK="app/build/outputs/apk/debug/app-debug.apk"

if [ ! -f "$APK" ]; then
    echo "ERROR: APK not found at $APK"
    exit 1
fi

# Install on connected device
echo "[2/2] Installing on device..."
adb install -r "$APK"

echo ""
echo "=== Done! Launch the app on your device. ==="
