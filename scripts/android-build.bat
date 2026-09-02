@echo off
setlocal enableextensions

set "SCRIPT_DIR=%~dp0"
pushd "%SCRIPT_DIR%\.." >nul

if /I "%~1"=="--help" goto :help
if /I "%~1"=="-h" goto :help

set "GAME_DIR=%~1"

echo === Flint Android Build ===
echo Engine:  %CD%
if defined GAME_DIR echo Game:    %GAME_DIR%
echo.

:: Check for cargo
where cargo >nul 2>&1
if errorlevel 1 (
    echo [error] Cargo is not available on PATH.
    echo Install Rust from https://rustup.rs/ and try again.
    goto :fail
)

:: Check for cargo-ndk
where cargo-ndk >nul 2>&1
if errorlevel 1 (
    echo [error] cargo-ndk is not installed.
    echo Install it with: cargo install cargo-ndk
    goto :fail
)

:: Check for adb (optional, only needed for install)
where adb >nul 2>&1
if errorlevel 1 (
    echo [warn] adb not found on PATH. Build will proceed but device install will be skipped.
    set "NO_ADB=1"
)

:: Build the native library
echo [1/3] Building flint-android with cargo ndk...
cargo ndk -t arm64-v8a -p flint-android --release
if errorlevel 1 goto :fail

:: Build the APK if android/gradlew exists
set "ANDROID_DIR=%CD%\android"
if not exist "%ANDROID_DIR%\gradlew.bat" (
    echo.
    echo Native library built successfully.
    echo No android/gradlew.bat found — skipping APK packaging.
    echo To build a full APK, set up the Android project in android/.
    goto :done
)

echo [2/3] Building APK...
pushd "%ANDROID_DIR%" >nul
if defined GAME_DIR (
    call gradlew.bat assembleDebug -PgameDir="%GAME_DIR%"
) else (
    call gradlew.bat assembleDebug
)
if errorlevel 1 (
    popd >nul
    goto :fail
)
popd >nul

set "APK=%ANDROID_DIR%\app\build\outputs\apk\debug\app-debug.apk"
if not exist "%APK%" (
    echo [error] APK not found at %APK%
    goto :fail
)

:: Install on connected device
if defined NO_ADB (
    echo [skip] adb not available — skipping device install.
    echo APK is at %APK%
    goto :done
)

echo [3/3] Installing on device...
adb install -r "%APK%"
if errorlevel 1 goto :fail

:done
echo.
echo === Android build completed successfully. ===
popd >nul
exit /b 0

:help
echo Usage: android-build.bat [game_dir]
echo.
echo Builds flint-android with cargo ndk, packages an APK via Gradle
echo (if available), and installs it on a connected device.
echo.
echo   game_dir    Path to the game project directory (optional).
echo.
echo Prerequisites:
echo   - Rust toolchain with aarch64-linux-android target
echo   - cargo-ndk (cargo install cargo-ndk)
echo   - Android NDK (set ANDROID_NDK_HOME)
echo   - adb on PATH (for device install)
popd >nul
exit /b 0

:fail
set "CODE=%ERRORLEVEL%"
echo.
echo [error] Command failed with exit code %CODE%.
popd >nul
exit /b %CODE%
