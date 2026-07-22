@echo off
title Install TrontSnap
REM One-click installer. Double-click me.
REM
REM Flow: this (Medium) window self-elevates ONCE to run bootstrap.ps1 (sign + install
REM into Program Files), WAITS for it, then launches TrontSnap from THIS non-elevated
REM window via `start`. That Medium/ShellExecute launch is what makes Windows grant the
REM installed, signed exe its uiAccess token (launching it from the elevated installer
REM would run it High and defeat the whole point).

net session >nul 2>&1
if %errorlevel%==0 goto :admin

echo Installing TrontSnap ^(one administrator prompt^)...
powershell -NoProfile -Command "Start-Process -FilePath '%~f0' -ArgumentList 'admin' -Verb RunAs -Wait"
echo.
echo Launching TrontSnap...
start "" "%ProgramFiles%\TrontSnap\trontsnap.exe"
echo.
echo ------------------------------------------------------------
echo  TrontSnap is installed and running.
echo  It now captures over elevated windows (TrontEQ, Task Mgr),
echo  drag-out still works, and it starts on login.
echo  Open it any time with "Launch TrontSnap" or the tray icon.
echo ------------------------------------------------------------
echo.
pause
exit /b

:admin
REM elevated branch: install only, do NOT launch (the Medium window above does that).
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0bootstrap.ps1"
exit /b
