@echo off
title Install TrontSnap
REM One-click installer. Double-click me.
REM
REM Flow: this (Medium) window self-elevates ONCE to run bootstrap.ps1 (remove the
REM portable install, sign, install into Program Files), WAITS for it, then launches
REM TrontSnap from THIS non-elevated window via `start`. That Medium/ShellExecute launch
REM is what makes Windows grant the installed, signed exe its uiAccess token (launching
REM it from the elevated installer would run it High and defeat the whole point).
REM
REM Pass "nopause" to skip the trailing prompt (for running from a terminal).

net session >nul 2>&1
if %errorlevel%==0 goto :admin
if /i "%~1"=="admin" goto :admin

echo Installing TrontSnap ^(one administrator prompt^)...
REM -PassThru + ExitCode: without it a cancelled UAC prompt, or any `exit 1` in
REM bootstrap.ps1, is invisible and we cheerfully print the success banner below.
REM
REM Only ONE argument is passed. An earlier version also forwarded %LOCALAPPDATA%
REM so the portable cleanup would target the right profile if UAC consent landed
REM on a different admin account, but every attempt to quote a path through
REM cmd -> powershell -> -ArgumentList -> cmd %~2 was fragile, and the version
REM that shipped was simply broken (inside PowerShell SINGLE quotes, \" is a
REM literal backslash-quote, not an escaped quote). The elevated branch falls
REM back to its own %LOCALAPPDATA%, which is correct for same-account UAC, i.e.
REM every normal case. Split-account installs just skip the portable cleanup,
REM which is cosmetic rather than harmful.
powershell -NoProfile -Command "$p = Start-Process -FilePath '%~f0' -ArgumentList 'admin' -Verb RunAs -Wait -PassThru; if ($null -eq $p) { exit 1 }; exit $p.ExitCode"
if errorlevel 1 (
    echo.
    echo ------------------------------------------------------------
    echo  INSTALL FAILED. Nothing was changed that you need to undo.
    echo  Details: C:\ProgramData\TrontSnap\bootstrap.log
    echo ------------------------------------------------------------
    echo.
    if /i "%~1"=="nopause" exit /b 1
    pause
    exit /b 1
)
echo.
echo Launching TrontSnap...
start "" "%ProgramFiles%\TrontSnap\trontsnap.exe"
echo.
echo ------------------------------------------------------------
echo  TrontSnap is installed and running.
echo  Bare PrintScreen now works over elevated windows
echo  ^(Task Manager, elevated terminals^), drag-out still works,
echo  and it starts on login.
echo  Open it any time with "Launch TrontSnap" or the tray icon.
echo ------------------------------------------------------------
echo.
if /i "%~1"=="nopause" exit /b
pause
exit /b

:admin
REM elevated branch: install only, do NOT launch (the Medium window above does that).
REM %2 carries the caller's real %LOCALAPPDATA% so the portable cleanup targets the
REM right profile even if UAC consent landed on a different admin account. It is empty
REM when this file is run directly from an already-elevated shell, so fall back to this
REM process's own value rather than resolving to a bare "\TrontSnap".
set "PORTABLE=%~2"
if "%PORTABLE%"=="" set "PORTABLE=%LOCALAPPDATA%"
REM bootstrap.ps1 sits BESIDE this file in the shipped bundle, but one directory
REM UP in the repo (it lives at the repo root, this file lives in packaging\).
REM Support both rather than only ever working from dist\.
set "BOOT=%~dp0bootstrap.ps1"
if not exist "%BOOT%" set "BOOT=%~dp0..\bootstrap.ps1"
if not exist "%BOOT%" (
    echo ERROR: cannot find bootstrap.ps1 next to this file or one level up.
    exit /b 1
)
powershell -NoProfile -ExecutionPolicy Bypass -File "%BOOT%" -PortableDir "%PORTABLE%\TrontSnap"
exit /b %errorlevel%
