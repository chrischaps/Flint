@echo off
REM All the logic is in build-and-install.sh (#764); this runs it in Git Bash.
call "%~dp0sh.cmd" "%~dp0build-and-install.sh" %*
