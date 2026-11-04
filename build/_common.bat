@echo off
REM ============================================================================
REM _common.bat - shared setup for the numbered build scripts (ASCII only in
REM               messages: cmd.exe cannot print UTF-8 reliably)
REM ----------------------------------------------------------------------------
REM Sourced via "call" by every numbered step and by full.bat / build.bat.
REM
REM Layout reminder (we live in <project>\main\build\):
REM   %SCRIPT_DIR%   = <project>\main\build\
REM   %MAIN_DIR%     = <project>\main          <- crate root, Cargo.toml here
REM   %PROJECT_ROOT% = <project>               <- .temp\, .build\, .3rd\, RuPix-Decoder\
REM     (RuPix-Decoder\ is a SEPARATE repo; its sources stay there, its build
REM      intermediates go to .temp\decoder-target - see its .cargo\config.toml)
REM
REM Build outputs:
REM   cargo intermediate -> %PROJECT_ROOT%\.temp\target       (main\.cargo\config.toml)
REM   ext  intermediate  -> %PROJECT_ROOT%\.temp\decoder-target (pinned by RuPix-Decoder\workspace\.cargo\config.toml)
REM   shipped exe        -> %PROJECT_ROOT%\.build\rupix.exe   (step 6)
REM   shipped decoders    -> %PROJECT_ROOT%\.build\decoders\    (step 3)
REM
REM IMPORTANT: no setlocal here on purpose - the caller needs the variables.
REM ============================================================================

REM ---- rust toolchain pin (K31, host side) ----------------------------------
REM This machine has rustup installed but NO default toolchain, so a bare
REM "cargo ..." fails with "rustup could not choose a version of cargo to run"
REM and 1_check_env.bat then reads an empty version string (looks like "cargo
REM found" while nothing was actually parsed). Mirror the value already pinned
REM by RuPix-Decoder\workspace\tools\toolchain.ps1 so host and decoder repos agree.
REM Only set it when absent: a caller can always override, and this touches no
REM global rustup config.
if not defined RUSTUP_TOOLCHAIN set "RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-msvc"

set "SCRIPT_DIR=%~dp0"
set "MAIN_DIR=%SCRIPT_DIR%.."
set "PROJECT_ROOT=%MAIN_DIR%\.."

REM Normalize away the "..\" segments so every derived path and all log output
REM shows a clean absolute path.
for %%A in ("%PROJECT_ROOT%") do set "PROJECT_ROOT=%%~fA"
for %%A in ("%MAIN_DIR%") do set "MAIN_DIR=%%~fA"

set "TEMP_DIR=%PROJECT_ROOT%\.temp"
set "TARGET_DIR=%TEMP_DIR%\target"
set "TARGET_EXT_DIR=%TEMP_DIR%\decoder-target"
set "OUTPUT_DIR=%PROJECT_ROOT%\.build"
set "DECODER_DIR=%OUTPUT_DIR%\decoders"
set "EXE=%OUTPUT_DIR%\rupix.exe"
set "EXT_DIR=%PROJECT_ROOT%\RuPix-Decoder"

REM ---- python (step 2 scans / step 6 deploy.py) -----------------------------
set "PY_EXE="
for %%A in (python.exe python3.exe) do if not defined PY_EXE for %%B in ("%%~$PATH:A") do set "PY_EXE=%%~B"

REM ---- powershell (step 3 builds the decoder DLLs via RuPix-Decoder\build.ps1) --
set "PS_EXE="
if defined SystemRoot if exist "%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe" set "PS_EXE=%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe"
if not defined PS_EXE set "PS_EXE=powershell.exe"


REM ---- source-path fingerprint (no host paths in shipped binaries) ----------
REM rustc bakes absolute paths into panic locations and debug info, so the exe
REM and every DLL would carry "<project>\main\src\.." plus
REM "%USERPROFILE%\.rustup\..". A portable build must not ship a map of the
REM author's disk. --remap-path-prefix is a plain prefix replacement:
REM   %PROJECT_ROOT% -> .   (anything inside the workspace)
REM   %USERPROFILE%  -> ~   (rustup toolchain + cargo registry)
REM NOTE: defining RUSTFLAGS REPLACES the rustflags from .cargo\config.toml
REM wholesale, so +crt-static has to be repeated here - same trap as the one
REM documented in build\size_map.py.
REM NOTE: it invalidates cargo's fingerprint, so the first build after this
REM change is a full one (~9 min); later builds are normal again.
REM Only set when absent: a caller can always override.
if not defined USERPROFILE set "USERPROFILE=%SystemDrive%\Users\%USERNAME%"
if not defined RUSTFLAGS set "RUSTFLAGS=-C target-feature=+crt-static --remap-path-prefix=%PROJECT_ROOT%=. --remap-path-prefix=%USERPROFILE%=~"

exit /b 0
