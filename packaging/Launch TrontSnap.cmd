@echo off
REM Launch the installed TrontSnap at Medium integrity (via ShellExecute `start`), which
REM is what makes Windows grant its uiAccess token. TrontSnap also starts on login.
if not exist "%ProgramFiles%\TrontSnap\trontsnap.exe" (
    echo TrontSnap isn't installed yet. Double-click "Install TrontSnap" first.
    echo.
    pause
    exit /b
)
start "" "%ProgramFiles%\TrontSnap\trontsnap.exe"
