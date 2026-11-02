@echo off
REM ============================================================================
REM full.bat - the one script to run (double-click this on a fresh checkout)
REM ----------------------------------------------------------------------------
REM Runs the whole pipeline in order, stopping at the first failure:
REM
REM   [1/6] 1_check_env.bat   verify the toolchain (read-only)
REM   [2/6] 2_scan.bat        static gates: layering_scan + hygiene_scan + cargo fmt
REM                           --check + cargo clippy -D warnings + structure_gen --check
REM   [3/6] 3_build_ext.bat   build+test+publish decoder DLLs (.build\decoders)
REM   [4/6] 4_build.bat       cargo build --release  -> .temp\target
REM   [5/6] 5_test.bat        test gate - FAILING TESTS STOP THE PIPELINE
REM   [6/6] 6_deploy.bat      copy exe into <project>\.build\ + verify decoders + local catalogue
REM
REM Each step is also runnable on its own - handy when only one thing changed:
REM   * new machine / no toolchain  -> 1_check_env.bat
REM   * code moved between layers   -> 2_scan.bat  (must be green before commit)
REM   * decoder changed              -> 3_build_ext.bat
REM   * iterating on host code       -> 4_build.bat
REM   * before snapshotting          -> 5_test.bat
REM   * need a runnable .build\      -> 6_deploy.bat
REM
REM Environment switches:
REM   RUPIX_SKIP_TESTS=1     skip step 5 (quick local loop)
REM   RUPIX_CARGO_ARGS       extra cargo args for step 4
REM
REM NOTE: main\build\ is SOURCE (it is part of the project tree). The shipped
REM binary goes to <project>\.build\ - one level up, outside main\.
REM ============================================================================

setlocal enabledelayedexpansion
set "RUPIX_CHAINED=1"

call "%~dp0_common.bat"
if %ERRORLEVEL% neq 0 (
    pause
    exit /b 1
)

echo ============================================================================
echo   RuPix full build
echo   crate  : %MAIN_DIR%
echo   target : %TARGET_DIR%
echo   output : %EXE%
echo ============================================================================
echo.

call "%SCRIPT_DIR%1_check_env.bat"
if !ERRORLEVEL! neq 0 (
    echo.
    echo   pipeline stopped at step 1 ^(toolchain^)
    pause
    exit /b 1
)

call "%SCRIPT_DIR%2_scan.bat"
if !ERRORLEVEL! neq 0 (
    echo.
    echo   pipeline stopped at step 2 ^(static gates^)
    pause
    exit /b 1
)

call "%SCRIPT_DIR%3_build_ext.bat"
if !ERRORLEVEL! neq 0 (
    echo.
    echo   pipeline stopped at step 3 ^(RuPix-Decoder^)
    pause
    exit /b 1
)

call "%SCRIPT_DIR%4_build.bat"
if !ERRORLEVEL! neq 0 (
    echo.
    echo   pipeline stopped at step 4 ^(build^)
    pause
    exit /b 1
)

call "%SCRIPT_DIR%5_test.bat"
if !ERRORLEVEL! neq 0 (
    echo.
    echo   pipeline stopped at step 5 ^(tests^)
    pause
    exit /b 1
)

call "%SCRIPT_DIR%6_deploy.bat"
if !ERRORLEVEL! neq 0 (
    echo.
    echo   pipeline stopped at step 6 ^(deploy^)
    pause
    exit /b 1
)

echo.
echo ======================================
echo    build complete
echo ======================================
echo.
echo   %EXE%
echo.
if not "%RUPIX_CHAINED%"=="1" pause
exit /b 0
