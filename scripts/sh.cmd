@echo off
REM Run one of Flint's bash scripts from cmd or Explorer (#764). Every helper
REM script has one implementation, in bash; each .bat is a line that calls this.
REM
REM   scripts\sh.cmd <script.sh> [args...]
REM
REM Finds Git for Windows' bash.exe. Not the first `bash` on PATH: on Windows
REM that is often C:\Windows\System32\bash.exe, WSL's, which sees a different
REM filesystem and toolchain. FLINT_BASH overrides the search.
setlocal enableextensions

set "BASH_EXE=%FLINT_BASH%"
if not defined BASH_EXE if exist "%ProgramFiles%\Git\bin\bash.exe" set "BASH_EXE=%ProgramFiles%\Git\bin\bash.exe"
if not defined BASH_EXE if exist "%ProgramFiles(x86)%\Git\bin\bash.exe" set "BASH_EXE=%ProgramFiles(x86)%\Git\bin\bash.exe"
if not defined BASH_EXE if exist "%LOCALAPPDATA%\Programs\Git\bin\bash.exe" set "BASH_EXE=%LOCALAPPDATA%\Programs\Git\bin\bash.exe"
if not defined BASH_EXE for /f "delims=" %%G in ('where git 2^>nul') do if not defined BASH_EXE if exist "%%~dpG..\bin\bash.exe" set "BASH_EXE=%%~dpG..\bin\bash.exe"

if not defined BASH_EXE (
    echo [error] Git for Windows' bash.exe was not found. Install Git from https://git-scm.com/
    echo         or set FLINT_BASH to a bash.exe.
    exit /b 1
)

"%BASH_EXE%" %*
exit /b %ERRORLEVEL%
