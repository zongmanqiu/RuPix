@echo off
REM ============================================================================
REM 6_deploy.bat - assemble <project>\.build\ from the cargo output
REM ----------------------------------------------------------------------------
REM deploy.py does the work (it is source, shipped in this directory):
REM   * copy .temp\target\release\rupix.exe -> .build\rupix.exe
REM     (atomic .new/.old swap: the exe may be RUNNING, and a mapped image
REM      handle refuses plain overwrite - the classic "cannot copy" trap)
REM   * MD5 + size cross-check: proves the deployed exe is the one just built
REM     (the exe is NOT reproducible - the PE header carries a build timestamp -
REM      so MD5 only proves "this is the file from this build", not code identity
REM      across builds)
REM   * inventory of .build\decoders\*.dll with an ABI/version sanity check
REM   * copy RuPix-Decoder\decoders.json -> .build\docs\decoders.json (the host reads
REM     this local catalogue when the decoders page opens; refresh updates it)
REM
REM Decoders are published by step 3 (RuPix-Decoder\build.ps1), NOT here. This step
REM only verifies they are present - a .build without decoders opens nothing.
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
echo    RuPix  [6/6] deploy to .build\
echo ======================================
echo.

if not defined PY_EXE (
    echo   [ERROR] python not found - cannot run deploy.py
    echo           install python and put it on PATH, then re-run this step
    pause
    exit /b 1
)

cd /d "%MAIN_DIR%"

"%PY_EXE%" "%SCRIPT_DIR%deploy.py"
if !ERRORLEVEL! neq 0 (
    echo   [ERROR] deploy.py failed - see the report above
    pause
    exit /b 1
)

echo.
echo   output: %EXE%
echo.

cd /d "%SCRIPT_DIR%"
if not "!RUPIX_CHAINED!"=="1" pause
exit /b 0
