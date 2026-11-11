@echo off
REM Build and publish the book. All the logic is in publish.sh (#764).
REM Usage: docs\publish.bat [--skip-rustdoc] [--docs-repo <dir>]
call "%~dp0..\scripts\sh.cmd" "%~dp0publish.sh" --publish %*
