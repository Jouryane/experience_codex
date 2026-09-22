@echo off
rem Double-click entry point: the REBUILT Codex, in the original desktop UI.
rem
rem The UI is the installed app's window; the backend it drives is our fork and
rem the home it uses is the fork's own, so nothing is shared with the installed
rem Codex. See start-codex-ui.ps1 for the mechanism.
rem
rem Windows PowerShell is used (always present); `pwsh` may not be installed.
setlocal
set "PS=%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe"
"%PS%" -NoProfile -ExecutionPolicy Bypass -File "%~dp0start-codex-ui.ps1" %*
set "EXITCODE=%ERRORLEVEL%"
if not "%EXITCODE%"=="0" (
    echo.
    echo [exit %EXITCODE%] press any key to close...
    pause >nul
)
exit /b %EXITCODE%
