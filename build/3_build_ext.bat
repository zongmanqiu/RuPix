@echo off
REM ============================================================================
REM 3_build_ext.bat - build and publish the decoder DLLs (RuPix-Decoder workspace)
REM ----------------------------------------------------------------------------
REM Wraps RuPix-Decoder\build.ps1, which does the full RuPix-Decoder cycle:
REM   cargo build --release --workspace
REM   cargo test  --release --workspace   (failure = no publish)
REM   publish <id>_<ver>.dll -> <project>\.temp\decoder-dist\
REM   mirror the same files into <project>\.build\decoders\ (host load dir)
REM
REM The RuPix-Decoder workspace is a SEPARATE cargo workspace; its intermediates go
REM to .temp\decoder-target (pinned by RuPix-Decoder\workspace\.cargo\config.toml, kept out of
REM the source repo). Nothing from it links into the exe.
REM
REM NOTE: RuPix-Decoder\build.ps1 is ASCII-only by policy; do not localize it here.
REM
REM Environment switch:
REM   RUPIX_CHAINED=1  suppress the final "press any key" (set by full.bat)
REM ============================================================================

setlocal enabledelayedexpansion

call "%~dp0_common.bat"
if %ERRORLEVEL% neq 0 (
    if not "!RUPIX_CHAINED!"=="1" pause
    exit /b 1
)

echo ======================================
echo    RuPix  [3/6] build decoder DLLs
echo ======================================
echo.

cd /d "%EXT_DIR%"
if !ERRORLEVEL! neq 0 (
    echo   [ERROR] cannot cd to %EXT_DIR%
    if not "!RUPIX_CHAINED!"=="1" pause
    exit /b 1
)

"%PS_EXE%" -NoProfile -ExecutionPolicy Bypass -File "%EXT_DIR%\build.ps1"
if !ERRORLEVEL! neq 0 (
    echo.
    echo   [ERROR] RuPix-Decoder build/test/publish failed
    if not "!RUPIX_CHAINED!"=="1" pause
    exit /b 1
)

echo.
echo   decoders published to %DECODER_DIR%
echo.

cd /d "%SCRIPT_DIR%"
if not "!RUPIX_CHAINED!"=="1" pause
exit /b 0
