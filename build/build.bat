@echo off
REM ============================================================================
REM build.bat - backwards-compatible one-shot entry point
REM ----------------------------------------------------------------------------
REM Historical name kept because README / AGENTS / older notes reference
REM "main\build\build.bat". It now just delegates to full.bat (the six-step
REM pipeline: check_env -> scan -> build_ext -> build -> test -> deploy).
REM
REM See build_readme.md for the pipeline contract and the per-step scripts.
REM ============================================================================

call "%~dp0full.bat"
exit /b %ERRORLEVEL%
