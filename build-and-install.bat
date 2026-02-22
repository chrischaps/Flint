@echo off
setlocal enableextensions

set "SCRIPT_DIR=%~dp0"
pushd "%SCRIPT_DIR%" >nul

if /I "%~1"=="--help" goto :help
if /I "%~1"=="-h" goto :help

where cargo >nul 2>&1
if errorlevel 1 (
    echo [error] Cargo is not available on PATH.
    echo Install Rust from https://rustup.rs/ and try again.
    popd >nul
    exit /b 1
)

echo [1/3] Building workspace in release mode...
cargo build --workspace --release --locked
if errorlevel 1 goto :fail

echo [2/3] Installing flint CLI...
cargo install --path crates\flint-cli --force --locked
if errorlevel 1 goto :fail

echo [3/3] Installing flint player...
cargo install --path crates\flint-player --force --locked
if errorlevel 1 goto :fail

echo.
echo Build and install completed successfully.
echo Installed binaries are in %USERPROFILE%\.cargo\bin
popd >nul
exit /b 0

:help
echo Usage: build-and-install.bat
echo.
echo Builds Flint in release mode and installs:
echo   - flint
echo   - flint-player
echo.
echo Binaries are installed to %USERPROFILE%\.cargo\bin.
popd >nul
exit /b 0

:fail
set "CODE=%ERRORLEVEL%"
echo.
echo [error] Command failed with exit code %CODE%.
popd >nul
exit /b %CODE%
