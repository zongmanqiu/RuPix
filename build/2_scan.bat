@echo off
REM ============================================================================
REM 2_scan.bat - run the static gates (read-only: no build artifacts)
REM ----------------------------------------------------------------------------
REM Checks, in order:
REM   layering_scan.py - five-layer direction rules + barred crate imports.
REM                      core(0) -> platform(1) -> decoders(2) -> ui(3) -> runtime(4);
REM                      a lower layer may never name a higher one.
REM   hygiene_scan.py  - hardcoded paths, debug leftovers, unfinished markers,
REM                      FFI boundaries without a guard, production panic sources
REM                      (unwrap/expect/panic!/todo!, AGENTS.md section 8.1 red line;
REM                      an exemption needs the marker comment right above the line,
REM                      kept ASCII here so cmd's codepage cannot garble it).
REM   cargo fmt --check- formatting gate. It was listed in AGENTS.md section 9
REM                      but NO step ever ran it, so drift sat undetected until
REM                      2026-10-17 (two files had drifted, one of them in the ABI
REM                      repo where fmt churn also desyncs the vendor snapshot).
REM                      Read-only: --check never rewrites sources.
REM   cargo clippy -D warnings - the lint gate (AGENTS.md section 9 row 4). It
REM                      was listed and never run, exactly like the fmt row was;
REM                      `-D warnings` is what turns "exit 0" into a judgement,
REM                      because plain clippy exits 0 *with* warnings.
REM   dump_sym.py --selftest - the crash resolver's own assertions (published
REM                      decoder name -> CodeView pdb name, report parsing). It is
REM                      the tool you reach for when a crash report arrives; it is
REM                      never run otherwise, so it gets a gate instead of trust.
REM   structure_gen.py --check - the structure doc (top-level "directory
REM                      structure" .md, the retrieval entry point): its counters and
REM                      its generated symbol index must equal what src/ says today.
REM                      Read-only too; --write is the fix. This is the gate that
REM                      keeps the doc from rotting the way hand-edited numbers did
REM                      (it claimed 46 files / 18,234 lines; disk had 45 / 19,108).
REM   cargo doc - broken intra-doc links (m282). The six checks above read code,
REM                      never doc comments, so 54 broken [`link`] targets sat in
REM                      src/ unnoticed - rustdoc resolves them only when someone
REM                      builds docs, which nobody does day to day. -D turns that
REM                      into a judgement; -A rustdoc::private_intra_doc_links
REM                      drops the "public doc links to a private item" noise
REM                      (that one is not a defect). It writes HTML under
REM                      %TARGET_DIR%\doc (inside .temp, never in main/).
REM
REM Either scanner reporting a violation makes this script return 1, which
REM stops full.bat before it builds or deploys anything.
REM
REM Both scanners resolve their paths from __file__, so the CWD here does not
REM matter - they always scan <project>\main\src.
REM
REM HISTORY: this file was accidentally left EMPTY, so full.bat step 2 always
REM "passed" while running nothing. A gate that cannot fail is not a gate -
REM hence the explicit hard error below when python is missing.
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
echo    RuPix  [2/6] static gates
echo ======================================
echo.

REM A missing python must FAIL, not silently skip. Skipping here is exactly
REM how the empty-file version of this script hid its own absence.
if not defined PY_EXE (
    echo   [ERROR] python not found on PATH - static gates cannot run.
    echo           install python or fix PATH. This step is NOT optional:
    echo           layering and hygiene violations must never reach a build.
    echo.
    cd /d "%SCRIPT_DIR%"
    if not "!RUPIX_CHAINED!"=="1" pause
    exit /b 1
)

echo   python : %PY_EXE%
echo.
echo [1/7] layering scan...
"%PY_EXE%" "%~dp0layering_scan.py"
if !ERRORLEVEL! neq 0 (
    echo.
    echo   [ERROR] layering scan failed - see the violations above
    echo.
    cd /d "%SCRIPT_DIR%"
    if not "!RUPIX_CHAINED!"=="1" pause
    exit /b 1
)

echo [2/7] hygiene scan...
"%PY_EXE%" "%~dp0hygiene_scan.py"
if !ERRORLEVEL! neq 0 (
    echo.
    echo   [ERROR] hygiene scan failed - see the violations above
    echo.
    cd /d "%SCRIPT_DIR%"
    if not "!RUPIX_CHAINED!"=="1" pause
    exit /b 1
)

REM Rust formatting gate. Runs from MAIN_DIR so .cargo\config.toml resolves and
REM only this workspace is checked (the plug-in repo has its own gate). cargo is
REM pinned by _common.bat via RUSTUP_TOOLCHAIN (K31); a missing cargo fails the
REM step rather than skipping it - same rule as python above.
where cargo >nul 2>&1
if errorlevel 1 (
    echo   [ERROR] cargo not found on PATH - formatting gate cannot run.
    cd /d "%SCRIPT_DIR%"
    if not "!RUPIX_CHAINED!"=="1" pause
    exit /b 1
)
echo [3/7] cargo fmt --check...
pushd "%MAIN_DIR%"
cargo fmt --check
set "FMT_RC=!ERRORLEVEL!"
popd
if not "!FMT_RC!"=="0" (
    echo.
    echo   [ERROR] cargo fmt --check failed - run "cargo fmt" in %MAIN_DIR% first
    echo.
    cd /d "%SCRIPT_DIR%"
    if not "!RUPIX_CHAINED!"=="1" pause
    exit /b 1
)

REM Rust lint gate. AGENTS.md section 9 row 4 promised "zero warning" but NO step
REM ever ran clippy, so warnings piled up silently (the same 列而不行 mistake the
REM fmt row was on 2026-10-17). `-D warnings` is the part that makes it a gate:
REM plain `cargo clippy` exits 0 WITH warnings, so "exit 0" proves nothing.
REM Read-only like fmt; it does NOT produce a binary (K63 - deploy still needs 4_build).
echo [4/7] cargo clippy -D warnings...
pushd "%MAIN_DIR%"
cargo clippy --all-targets --release -- -D warnings
set "CLIPPY_RC=!ERRORLEVEL!"
popd
if not "!CLIPPY_RC!"=="0" (
    echo.
    echo   [ERROR] cargo clippy -D warnings failed - fix the lints above
    echo.
    cd /d "%SCRIPT_DIR%"
    if not "!RUPIX_CHAINED!"=="1" pause
    exit /b 1
)

REM Structure-doc freshness. Read-only --check: the doc's counters and its
REM generated symbol index must match src/ right now. A doc that lies about where
REM a function lives is worse than no doc, so this fails the pipeline like any
REM other gate - the fix is to run --write, never to hand-edit the numbers.
echo [5/7] structure doc freshness...
"%PY_EXE%" "%~dp0structure_gen.py" --check
if !ERRORLEVEL! neq 0 (
    echo.
    echo   [ERROR] structure doc drifted from src - fix with:
    echo           python build\structure_gen.py --write
    echo.
    cd /d "%SCRIPT_DIR%"
    if not "!RUPIX_CHAINED!"=="1" pause
    exit /b 1
)

REM Crash-forensics resolver self-test. dump_sym.py is the LAST step of the
REM evidence chain and nobody runs it day to day, so a silent regression there
REM (e.g. the published-name -> CodeView-name mapping) surfaces only at 3 a.m.
REM when a report has to be read. Its --selftest needs no symbol archive and no
REM crash fixture, so it runs everywhere in under a second (m131).
echo [6/7] dump_sym.py selftest...
"%PY_EXE%" "%~dp0dump_sym.py" --selftest
if !ERRORLEVEL! neq 0 (
    echo.
    echo   [ERROR] dump_sym.py selftest failed - the crash resolver is broken
    echo.
    cd /d "%SCRIPT_DIR%"
    if not "!RUPIX_CHAINED!"=="1" pause
    exit /b 1
)

REM Doc-link gate (m282). Reads comments, not code: every [`X`] target must
REM resolve. --document-private-items is deliberate - it widens the check to
REM private items instead of hiding them.
echo [7/7] cargo doc - broken intra-doc links...
pushd "%MAIN_DIR%"
set "RUSTDOCFLAGS=-D rustdoc::broken_intra_doc_links -A rustdoc::private_intra_doc_links"
cargo doc --no-deps --lib --document-private-items
set "DOC_RC=!ERRORLEVEL!"
popd
if not "!DOC_RC!"=="0" (
    echo.
    echo   [ERROR] broken intra-doc link in src/ comments - point it at the real
    echo           path or degrade it to a plain code span
    echo.
    cd /d "%SCRIPT_DIR%"
    if not "!RUPIX_CHAINED!"=="1" pause
    exit /b 1
)

echo.
echo   static gates passed
echo.

cd /d "%SCRIPT_DIR%"
if not "!RUPIX_CHAINED!"=="1" pause
exit /b 0
