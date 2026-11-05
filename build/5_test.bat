@echo off
REM ============================================================================
REM 5_test.bat - run the test gate
REM ----------------------------------------------------------------------------
REM This is a GATE, not a report: a failing test makes this script return 1,
REM which stops full.bat before it deploys anything.
REM
REM One complete cargo test --release run covers unit tests inside src/ and
REM integration tests (tests/*.rs, incl. docs_consistency.rs = doc-rot gate).
REM Do not repeat --lib afterwards: the complete run already includes it.
REM
REM NOTE: never pass "--lib" alone as the whole gate - it silently skips
REM tests/*.rs, which is where docs_consistency lives.
REM
REM NOTE: "-- --test-threads=1" is NOT optional on this machine. The default
REM multi-threaded runner links/executes test binaries in parallel and the
REM antivirus holds a lock on the freshly-linked binaries, which stalls the run
REM (it looks like a hang). Single-threaded finishes the whole suite in seconds.
REM
REM Environment switches:
REM   RUPIX_SKIP_TESTS=1  skip everything (quick local loop)
REM   RUPIX_CHAINED=1     suppress the final "press any key" (set by full.bat)
REM ============================================================================

setlocal enabledelayedexpansion

call "%~dp0_common.bat"
if %ERRORLEVEL% neq 0 (
    if not "!RUPIX_CHAINED!"=="1" pause
    exit /b 1
)

echo ======================================
echo    RuPix  [5/6] test gate
echo ======================================
echo.

if "%RUPIX_SKIP_TESTS%"=="1" (
    echo   skipped - RUPIX_SKIP_TESTS=1
    echo.
    cd /d "%SCRIPT_DIR%"
    if not "!RUPIX_CHAINED!"=="1" pause
    exit /b 0
)

cd /d "%MAIN_DIR%"
set "CARGO_INCREMENTAL=0"
set "TEMP=%TEMP_DIR%"
set "TMP=%TEMP_DIR%"
if not exist "%TEMP_DIR%" mkdir "%TEMP_DIR%"
REM Test fixtures land in the repo .temp (core::paths::test_scratch_root), named with the
REM test process id. Each run adds a new set, so sweep the whole subtree here - at this point
REM nothing is running yet, so it cannot pull a live fixture out from under a test process.
if exist "%TEMP_DIR%\rupix-test" rd /s /q "%TEMP_DIR%\rupix-test"

echo [1/2] layering + hygiene scan...
python "%~dp0layering_scan.py"
if !ERRORLEVEL! neq 0 (
    echo.
    echo   [ERROR] layering scan failed
    if not "!RUPIX_CHAINED!"=="1" pause
    exit /b 1
)
python "%~dp0hygiene_scan.py"
if !ERRORLEVEL! neq 0 (
    echo.
    echo   [ERROR] hygiene scan failed
    if not "!RUPIX_CHAINED!"=="1" pause
    exit /b 1
)

echo.
echo [2/2] unit + integration tests...
cargo test --release -- --test-threads=1
if !ERRORLEVEL! neq 0 (
    echo.
    echo   [ERROR] tests failed
    if not "!RUPIX_CHAINED!"=="1" pause
    exit /b 1
)

echo.
echo   all tests passed
echo.

cd /d "%SCRIPT_DIR%"
if not "!RUPIX_CHAINED!"=="1" pause
exit /b 0
