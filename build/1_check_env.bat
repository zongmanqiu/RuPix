@echo off
REM ============================================================================
REM 1_check_env.bat - verify the build toolchain is installed (read-only)
REM ----------------------------------------------------------------------------
REM Checks, in order:
REM   cargo / rustc  - must be the MSVC host (x86_64-pc-windows-msvc), not GNU
REM   python         - step 2 (layering_scan / hygiene_scan) and step 6 need it
REM   powershell     - step 3 runs RuPix-Decoder\build.ps1
REM   .3rd\windui    - the local path dependency. MISSING IS AN ERROR here
REM                    (unlike CurSeen: our .3rd is a checked-in local copy,
REM                    there is no download step to repair it).
REM   RuPix-Decoder\     - the decoder workspace; missing members fail step 3.
REM
REM This script changes NOTHING on disk. It only reports.
REM
REM Environment switch:
REM   RUPIX_CHAINED=1  suppress the final "press any key" (set by full.bat)
REM ============================================================================

setlocal enabledelayedexpansion

call "%~dp0_common.bat"
if %ERRORLEVEL% neq 0 (
    pause
    exit /b 1
)

echo ======================================
echo    RuPix  [1/6] check toolchain
echo ======================================
echo.

set "ENV_FAIL=0"

REM ---- cargo / rustc --------------------------------------------------------
REM RUSTUP_TOOLCHAIN is pinned by _common.bat (K31): this machine has no default
REM toolchain, so without it cargo errors out and BOTH loops below would parse an
REM empty string while still printing [OK]. Fail loudly instead.
where cargo >nul 2>&1
if !ERRORLEVEL! neq 0 (
    echo   [ERROR] cargo not found - install Rust from https://rustup.rs
    set "ENV_FAIL=1"
) else (
    for /f "delims=" %%v in ('cargo --version 2^>nul') do if not defined CARGO_VER set "CARGO_VER=%%v"
    if "!CARGO_VER!"=="" (
        echo   [ERROR] cargo produced no version string - toolchain not selectable
        echo           is RUSTUP_TOOLCHAIN set? _common.bat pins it; do not clear it.
        set "ENV_FAIL=1"
    ) else (
        echo   [OK] !CARGO_VER!
    )
    REM The MSVC toolchain is required (windui / windows crates).
    REM NOTE: deliberately at top level - a pipe inside a parenthesised block
    REM that also uses delayed expansion is a classic source of parse errors.
    set "RUST_HOST="
    for /f "delims=" %%h in ('rustc -vV 2^>nul ^| findstr /B /C:"host:"') do set "RUST_HOST=%%h"
    if "!RUST_HOST!"=="" (echo   [ERROR] rustc -vV gave no host line) else echo   [OK] !RUST_HOST!
)

set "RUST_HOST_OK=0"
if defined RUST_HOST if not "!RUST_HOST:pc-windows-msvc=!"=="!RUST_HOST!" set "RUST_HOST_OK=1"
if "%RUST_HOST_OK%"=="0" (
    echo   [ERROR] rust host is not MSVC - need stable-x86_64-pc-windows-msvc
    echo           this script pins RUSTUP_TOOLCHAIN via _common.bat; if that does
    echo           not fix it, install the toolchain: rustup toolchain install stable-x86_64-pc-windows-msvc
    set "ENV_FAIL=1"
)

REM ---- python (scans + deploy) ---------------------------------------------
if defined PY_EXE (
    echo   [OK] python   : %PY_EXE%
) else (
    echo   [ERROR] python not found on PATH - steps 2 and 6 need it
    set "ENV_FAIL=1"
)

REM ---- powershell (RuPix-Decoder build) ----------------------------------------
if exist "%PS_EXE%" (
    echo   [OK] powershell
) else (
    where powershell >nul 2>&1
    if !ERRORLEVEL! neq 0 (
        echo   [ERROR] powershell not found - step 3 cannot build decoder DLLs
        set "ENV_FAIL=1"
    ) else (
        echo   [OK] powershell
    )
)

REM ---- third-party path dependency (hard error: nothing repairs it) ---------
if exist "%PROJECT_ROOT%\.3rd\wind-ui-rust-0.23.0\Cargo.toml" (
    echo   [OK] .3rd\wind-ui-rust-0.23.0
) else (
    echo   [ERROR] missing .3rd\wind-ui-rust-0.23.0 - main\Cargo.toml path
    echo           dependency cannot resolve; restore it from a snapshot
    set "ENV_FAIL=1"
)

REM ---- RuPix-Decoder workspace --------------------------------------------------
if exist "%EXT_DIR%\workspace\Cargo.toml" (
    echo   [OK] RuPix-Decoder\ workspace
) else (
    echo   [ERROR] missing RuPix-Decoder\ workspace - decoder DLLs cannot be built
    set "ENV_FAIL=1"
)

echo.
echo   crate  : %MAIN_DIR%
echo   target : %TARGET_DIR%
echo   output : %EXE%
echo.

if "!ENV_FAIL!"=="1" (
    echo ======================================
    echo    toolchain incomplete - fix the [ERROR] lines above
    echo ======================================
    echo.
    cd /d "%SCRIPT_DIR%"
    if not "!RUPIX_CHAINED!"=="1" pause
    exit /b 1
)

echo   toolchain check passed
echo.

cd /d "%SCRIPT_DIR%"
if not "!RUPIX_CHAINED!"=="1" pause
exit /b 0
