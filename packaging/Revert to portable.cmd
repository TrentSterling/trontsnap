@echo off
title Revert TrontSnap to portable
REM Undoes the Program Files / uiAccess install. See revert-to-portable.ps1 for why.
REM Self-elevates once (killing a High-integrity process needs it), then relaunches
REM the portable exe from THIS non-elevated window so it runs at Medium and can drag.
REM
REM Pass "nopause" to skip the trailing prompt.

net session >nul 2>&1
if %errorlevel%==0 goto :admin
if /i "%~1"=="admin" goto :admin

echo Reverting TrontSnap to the portable build ^(one administrator prompt^)...
powershell -NoProfile -Command "Start-Process -FilePath '%~f0' -ArgumentList 'admin','%LOCALAPPDATA%' -Verb RunAs -Wait"
echo.
echo Launching the portable build...
start "" "%LOCALAPPDATA%\TrontSnap\trontsnap.exe"
echo.
echo ------------------------------------------------------------
echo  Back on the portable build, running at Medium integrity.
echo  Drag-out into terminals/Discord works again.
echo  Fullscreen capture is now Alt+PrtSc.
echo  ^(Ctrl+PrtSc region and Ctrl+Shift+PrtSc record are unchanged.^)
echo ------------------------------------------------------------
echo.
if /i "%~1"=="nopause" exit /b
pause
exit /b

:admin
set "PORTABLE=%~2"
if "%PORTABLE%"=="" set "PORTABLE=%LOCALAPPDATA%"
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0revert-to-portable.ps1" -PortableDir "%PORTABLE%\TrontSnap"
exit /b
