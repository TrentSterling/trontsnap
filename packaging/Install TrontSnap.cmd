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
powershell -NoProfile -Command "Start-Process -FilePath '%~f0' -ArgumentList 'admin','%LOCALAPPDATA%' -Verb RunAs -Wait"
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
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0bootstrap.ps1" -PortableDir "%PORTABLE%\TrontSnap"
exit /b
