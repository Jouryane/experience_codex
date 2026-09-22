@echo off
rem Double-click entry point for the rebuilt Codex (Experience embedded).
rem
rem Pure batch on purpose: no PowerShell, no environment prerequisites. The
rem fork creates its own canonical experience store on first use, so the only
rem thing worth adding here is the backup root for gate-taken write steps.
rem
rem All arguments are forwarded verbatim, so these all work:
rem   start-experience-codex.cmd
rem   start-experience-codex.cmd exec --skip-git-repo-check
rem   start-experience-codex.cmd experience html --open
setlocal

set "CODEX_EXE=%~dp0codex-main\codex-rs\target\debug\codex.exe"
if not exist "%CODEX_EXE%" (
    echo fork build not found: %CODEX_EXE%
    echo build it with: cd /d "%~dp0codex-main\codex-rs" ^&^& cargo build -p codex-cli
    echo.
    pause
    exit /b 2
)

rem The rebuilt Codex gets its OWN home. Sharing %USERPROFILE%\.codex with the
rem installed Codex desktop app makes the two fight over the SQLite state DB
rem ("migration 1 was previously applied but has been modified"), and mixes
rem their sessions, logs, plugin caches and experience stores.
if not defined CODEX_HOME set "CODEX_HOME=%USERPROFILE%\.codex-experience"
if not exist "%CODEX_HOME%\config.toml" (
    echo the fork's own home is not configured yet: %CODEX_HOME%
    echo run this once to build it from your real config:
    echo     powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0setup-fork-home.ps1"
    echo.
    pause
    exit /b 3
)

if not defined EXPERIENCE_GATE_BACKUP_ROOT set "EXPERIENCE_GATE_BACKUP_ROOT=%~dp0.experience-backups"

"%CODEX_EXE%" %*
set "EXITCODE=%ERRORLEVEL%"
if not "%EXITCODE%"=="0" (
    echo.
    echo [exit %EXITCODE%] press any key to close...
    pause >nul
)
exit /b %EXITCODE%
