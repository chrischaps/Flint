@echo off
REM Launcher for Flint docs build+publish. All logic lives in build.ps1.
REM Usage: docs\publish.bat [--skip-rustdoc]
set PS_FLAGS=-Publish
if /i "%~1"=="--skip-rustdoc" set PS_FLAGS=-Publish -SkipRustdoc
powershell -ExecutionPolicy Bypass -File "%~dp0build.ps1" %PS_FLAGS%
