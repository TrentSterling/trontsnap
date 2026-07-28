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
REM One argument only, and the exit code is checked. See the long note in
REM "Install TrontSnap.cmd": forwarding %LOCALAPPDATA% through cmd -> powershell
REM -> -ArgumentList could not be quoted reliably, and the elevated branch's own
REM %LOCALAPPDATA% is correct for same-account UAC anyway.
powershell -NoProfile -Command "$p = Start-Process -FilePath '%~f0' -ArgumentList 'admin' -Verb RunAs -Wait -PassThru; if ($null -eq $p) { exit 1 }; exit $p.ExitCode"
if errorlevel 1 (
    echo.
    echo  REVERT FAILED - see C:\ProgramData\TrontSnap\bootstrap.log
    echo.
    if /i "%~1"=="nopause" exit /b 1
    pause
    exit /b 1
)
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
set "REVERT=%~dp0revert-to-portable.ps1"
if not exist "%REVERT%" set "REVERT=%~dp0..\packaging\revert-to-portable.ps1"
if not exist "%REVERT%" (
    echo ERROR: cannot find revert-to-portable.ps1.
    exit /b 1
)
powershell -NoProfile -ExecutionPolicy Bypass -File "%REVERT%" -PortableDir "%PORTABLE%\TrontSnap"
REM Propagate the real exit code. A bare `exit /b` here discarded it, so the
REM non-elevated half saw success no matter what the script did, and printed the
REM "back on the portable build" banner over a failed revert.
exit /b %errorlevel%
