@echo off
REM All the logic is in android-build.sh (#764); this runs it in Git Bash.
call "%~dp0sh.cmd" "%~dp0android-build.sh" %*
