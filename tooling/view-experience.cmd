@echo off
rem Regenerate the read-only HTML report and open it in the default browser.
rem Pure batch, same reasoning as start-experience-codex.cmd: no PowerShell
rem dependency, and codex arguments (`--store`, `--out`) are forwarded as-is.
setlocal
set "CODEX_EXE=%~dp0codex-main\codex-rs\target\debug\codex.exe"
if not exist "%CODEX_EXE%" (
    echo fork build not found: %CODEX_EXE%
    echo build it with: cd /d "%~dp0codex-main\codex-rs" ^&^& cargo build -p codex-cli
    echo.
    pause
    exit /b 2
)

rem Read the fork's own store, not the installed app's. Override the store by
rem passing --store <path> after this wrapper's own arguments.
if not defined CODEX_HOME set "CODEX_HOME=%USERPROFILE%\.codex-experience"
"%CODEX_EXE%" experience html --store "%CODEX_HOME%\experience\store.json" --open %*
set EXITCODE=%ERRORLEVEL%
if not "%EXITCODE%"=="0" (
    echo.
    echo [exit %EXITCODE%] press any key to close...
    pause >nul
)
exit /b %EXITCODE%
