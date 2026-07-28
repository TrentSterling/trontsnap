# Undo the Program Files / uiAccess install and go back to the portable build.
#
# WHY YOU WOULD RUN THIS. uiAccess is granted by handing the process a raised
# token, and on an admin account that lands it at HIGH integrity. Windows blocks
# OLE drag-and-drop across an integrity boundary, so a High TrontSnap cannot drag
# a screenshot into a Medium terminal, Discord or browser. Measured 2026-07-27:
# trontsnap integrity=HIGH uiAccess=YES, terminals integrity=MEDIUM, drag dead.
#
# ShareX sidesteps this by having no uiAccess and no requestedExecutionLevel at
# all, so it runs Medium even from Program Files and drags fine. The cost is that
# a bare (modifier-less) PrtSc bind is not delivered while an elevated window has
# focus. The portable build makes the same trade and defaults Fullscreen to
# Alt+PrtSc so nothing is silently dead.
#
# Getting BOTH needs the hotkey to live in a separate uiAccess process from the
# UI; that helper is the next piece of work and supersedes this script.
#
# Elevated (killing a High-integrity process and writing Program Files both need
# it). Run via "Revert to portable.cmd".

param(
    [string]$PortableDir = (Join-Path $env:LOCALAPPDATA 'TrontSnap')
)

$ErrorActionPreference = 'Continue'
$root       = $PSScriptRoot
$installDir = Join-Path $env:ProgramFiles 'TrontSnap'
$dataDir    = 'C:\ProgramData\TrontSnap'
$sys32      = Join-Path $env:WINDIR 'System32'

function Log($m) {
    $line = "$((Get-Date).ToString('HH:mm:ss'))  $m"
    Write-Host $line
    if (Test-Path $dataDir) { Add-Content -Path (Join-Path $dataDir 'bootstrap.log') -Value $line -Encoding utf8 }
}

$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Host "ERROR: run this elevated ('Revert to portable.cmd' does that for you)." -ForegroundColor Red
    exit 1
}
Log "== revert to portable @ $(Get-Date) =="

# --- 1. the portable exe must exist BEFORE we tear anything down ----------------
# Search order covers the shipped bundle, a loose drop, and a plain repo checkout.
# The repo fallback matters: without it this script could never run from a clone,
# which made the only documented recovery path a guaranteed no-op.
$candidates = @(
    (Join-Path $root 'portable\trontsnap.exe'),
    (Join-Path $root 'trontsnap-portable.exe'),
    (Join-Path $root '..\target\release\trontsnap.exe'),
    (Join-Path $root '..\dist\portable\trontsnap.exe')
)
$srcPortable = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $srcPortable) {
    Log "ERROR: no portable exe found. Looked in:"
    $candidates | ForEach-Object { Log "         $_" }
    Log "       Build one with: cargo build --release"
    exit 1
}
$m = [System.Text.Encoding]::ASCII.GetString([System.IO.File]::ReadAllBytes($srcPortable))
if ($m -match 'uiAccess="true"') {
    Log "ERROR: $srcPortable IS a uiAccess build; that is the thing being reverted."
    Log "       Build the plain one with: cargo build --release"
    exit 1
}
Log "portable source: $srcPortable (uiAccess=false confirmed)"

# --- 2. stop BOTH processes -------------------------------------------------------
# The broker matters as much as the UI here. It is a uiAccess process holding the
# global hotkey registrations; if it survives, it keeps them, and the portable
# build relaunched at the end loses the race and comes up with silently dead
# hotkeys. It also locks its own exe, so step 4's directory removal would leave a
# half-deleted Program Files install behind.
& (Join-Path $sys32 'taskkill.exe') /IM trontsnap.exe /F 2>&1 | Out-Null
& (Join-Path $sys32 'taskkill.exe') /IM trontsnap-hotkeys.exe /F 2>&1 | Out-Null
$deadline = (Get-Date).AddSeconds(8)
while (((Get-Process trontsnap -ErrorAction SilentlyContinue) -or
        (Get-Process trontsnap-hotkeys -ErrorAction SilentlyContinue)) -and (Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 200
}
if (Get-Process trontsnap -ErrorAction SilentlyContinue) { Log "WARN: trontsnap.exe still running" }
if (Get-Process trontsnap-hotkeys -ErrorAction SilentlyContinue) { Log "WARN: trontsnap-hotkeys.exe still running (hotkeys may stay dead)" }
Log "stopped running instances"

# --- 3. put the portable exe back ------------------------------------------------
# Only the exe is written. The folder is ALSO the thumbnail cache
# (thumbs.rs -> dirs::cache_dir() -> %LOCALAPPDATA%\TrontSnap\thumbs), so it is
# created if missing and never wiped.
New-Item -ItemType Directory -Force -Path $PortableDir | Out-Null
$destPortable = Join-Path $PortableDir 'trontsnap.exe'
Copy-Item $srcPortable $destPortable -Force
# Hash, not Test-Path. On a re-run the destination already exists, so Test-Path
# passes even when Copy-Item failed (locked file, access denied), and step 4 would
# then delete the working Program Files install while leaving a STALE or partial
# portable exe behind as the only remaining binary. Same trap bootstrap.ps1 had.
if ((Get-FileHash $srcPortable).Hash -ne (Get-FileHash $destPortable -ErrorAction SilentlyContinue).Hash) {
    Log "ERROR: $destPortable does not match the source after copy; leaving the install alone"
    exit 1
}
Log "restored portable -> $destPortable"

# --- 4. remove the Program Files install ----------------------------------------
if (Test-Path $installDir) {
    Remove-Item $installDir -Recurse -Force -ErrorAction SilentlyContinue
    if (Test-Path $installDir) { Log "WARN: could not remove $installDir" } else { Log "removed $installDir" }
} else {
    Log "no Program Files install to remove"
}

# --- 5. point autostart back at the portable exe ---------------------------------
$run = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
Set-ItemProperty -Path $run -Name 'TrontSnap' -Value "`"$destPortable`" --startup"
Log "autostart -> $destPortable --startup"

Log "RESULT: reverted to portable. Fullscreen is Alt+PrtSc on this build; drag-out works again."
