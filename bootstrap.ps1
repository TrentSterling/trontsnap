# TrontSnap installer. Removes any portable install, then signs trontsnap.exe and
# installs it into %ProgramFiles%\TrontSnap so Windows will grant it uiAccess (a signed
# exe in a secure location).
#
# WHY uiAccess. TrontSnap binds its hotkeys with RegisterHotKey. Combos WITH modifiers
# (Ctrl+PrtSc, Ctrl+Shift+PrtSc) reach a Medium-integrity process even while an ELEVATED
# window has focus, but MODIFIER-LESS binds (bare PrtSc) do NOT. A/B tested 2026-07-23,
# reconfirmed 2026-07-26 against Task Manager. uiAccess sets the token flag that bypasses
# UIPI, so bare PrtSc works everywhere WITHOUT elevating TrontSnap: it stays Medium
# integrity and drag-out into Discord keeps working (confirmed live on v0.9.0).
#
# The PORTABLE build remains fully supported and needs none of this; it just defaults
# Fullscreen to Alt+PrtSc instead of bare PrtSc. See settings::DEFAULT_FULL_MODS.
#
# Single-phase, elevated, $PSScriptRoot-relative, idempotent. No test-signing, no reboot
# (those were only for TrontEQ's kernel-loaded APO DLL; TrontSnap is a plain user-mode exe
# that just needs a valid Authenticode signature chaining to a trusted root).
#
# Run via "Install TrontSnap.cmd" (it self-elevates and then launches the app at Medium so
# AppInfo actually grants uiAccess). Running bootstrap alone installs but does NOT launch.

param(
    # The portable install to clean up. Passed by Install TrontSnap.cmd from the
    # NON-elevated window, because if UAC consent lands on a different admin account
    # this script's own %LOCALAPPDATA% would point at the wrong profile.
    [string]$PortableDir = (Join-Path $env:LOCALAPPDATA 'TrontSnap'),

    # Permit installing an OLDER build over a newer one. Off by default; see the
    # downgrade guard in step 1. Only pass this if you actually mean to roll back.
    [switch]$AllowDowngrade
)

# 'Continue', not 'Stop': native tools (taskkill, signtool) write to stderr even on
# success, and under 'Stop' a redirected native stderr line becomes a terminating error
# in Windows PowerShell 5.1. Real failures are caught by the explicit checks below
# (Test-Path on the copy, signature Status -eq 'Valid').
$ErrorActionPreference = 'Continue'
$root       = $PSScriptRoot
$installDir = Join-Path $env:ProgramFiles 'TrontSnap'
$destExe    = Join-Path $installDir 'trontsnap.exe'
$dataDir    = 'C:\ProgramData\TrontSnap'
$logFile    = Join-Path $dataDir 'bootstrap.log'
$sys32      = Join-Path $env:WINDIR 'System32'

function Log($m) {
    $line = "$((Get-Date).ToString('HH:mm:ss'))  $m"
    Write-Host $line
    if (Test-Path $dataDir) { Add-Content -Path $logFile -Value $line -Encoding utf8 }
}

# --- must be elevated -----------------------------------------------------------
$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Host "ERROR: run this elevated ('Install TrontSnap.cmd' does that for you)." -ForegroundColor Red
    exit 1
}
New-Item -ItemType Directory -Force -Path $dataDir | Out-Null
Log "== TrontSnap install @ $(Get-Date) as $(whoami) =="

# --- 1. locate the exe to install (prebuilt beside script, or source build) -----
#
# ALWAYS BUILD when installing from source. This used to be
# `if (-not (Test-Path $srcExe)) { build }`, i.e. it skipped the build whenever
# ANY exe was already sitting in target\release, then installed that. Combined
# with a UI that displayed no version anywhere, a months-old binary could be
# installed and debugged as if it were current; a v0.16.0 install was found
# masquerading as v0.22.0 on 2026-07-31. cargo is incremental, so an
# unconditional build costs ~nothing when the tree is already up to date and is
# the only thing that makes "reinstall" actually mean "install what I wrote".
#
# The `--features uiaccess` flag is also gone from that build line: the feature
# was removed from Cargo.toml in v0.15.0, so the command errored out. The build
# branch was therefore BOTH skipped by default and broken when reached, which is
# why nothing ever surfaced the drift.
function Get-ExeVersion($path) {
    if (Test-Path $path) { (Get-Item $path).VersionInfo.FileVersion } else { $null }
}

$prebuilt = Join-Path $root 'trontsnap.exe'
$srcExe   = Join-Path $root 'target\release\trontsnap.exe'
if (Test-Path $prebuilt) {
    $srcBuilt = $prebuilt
    Log "layout: prebuilt ($prebuilt)"
} else {
    Log "building: cargo build --release (UI + hotkey broker)"
    Push-Location $root
    try {
        & cargo build --release 2>&1 | ForEach-Object { Log "  $_" }
        $uiExit = $LASTEXITCODE
        & cargo build --release -p trontsnap-hotkeys 2>&1 | ForEach-Object { Log "  $_" }
        $brokerExit = $LASTEXITCODE
    }
    finally { Pop-Location }
    if ($uiExit -ne 0)     { Log "ERROR: cargo build --release failed (exit $uiExit)"; exit 1 }
    if ($brokerExit -ne 0) { Log "ERROR: broker build failed (exit $brokerExit)"; exit 1 }
    if (-not (Test-Path $srcExe)) { Log "ERROR: $srcExe not found and the build did not produce it"; exit 1 }
    $srcBuilt = $srcExe
    Log "layout: source ($srcExe)"
}

# Say out loud what is replacing what. A silent no-op install is the failure
# mode this whole section exists to prevent, so make the version change visible
# in the log even when everything works.
$destExeVer = Get-ExeVersion $destExe
$srcExeVer  = Get-ExeVersion $srcBuilt
if ($destExeVer) {
    Log "version: installed v$destExeVer  ->  incoming v$srcExeVer"
    if ($destExeVer -eq $srcExeVer) { Log "  (same version number; the binary is still replaced byte-for-byte below)" }
} else {
    Log "version: no existing install  ->  incoming v$srcExeVer"
}

# --- DOWNGRADE GUARD -------------------------------------------------------------
#
# Installing an OLDER build over a newer one is never what you meant, and it is
# the exact trap this repo already sprung: `dist\` holds packaged copies
# (v0.15.0, v0.16.0, v0.17.0 as of 2026-07-31), and because `$root` comes from
# $PSScriptRoot, running a COPY of this script that sits beside one of them takes
# the "prebuilt" branch above and installs the fossil without building anything.
# That is how a v0.16.0 install ended up masquerading as current for four days
# while the tree was at v0.22.0.
#
# Refuse by default. A rollback is a legitimate thing to want, so -AllowDowngrade
# exists, but it has to be asked for out loud.
if ($destExeVer -and $srcExeVer) {
    $cmpOk = $false
    try { $older = ([version]$srcExeVer -lt [version]$destExeVer); $cmpOk = $true }
    catch { Log "  (version strings not comparable; skipping the downgrade check)" }
    if ($cmpOk -and $older) {
        if ($AllowDowngrade) {
            Log "WARN: DOWNGRADING v$destExeVer -> v$srcExeVer because -AllowDowngrade was passed."
        } else {
            Log "ERROR: refusing to DOWNGRADE the install from v$destExeVer to v$srcExeVer."
            Log "       Source of the older binary: $srcBuilt"
            Log "       You are probably running a copy of this script that sits next to a"
            Log "       packaged build (check dist\). Run bootstrap.ps1 from the REPO ROOT to"
            Log "       build and install current source, or pass -AllowDowngrade to roll back."
            exit 1
        }
    }
}

# The UI must NOT have uiAccess. It would be raised to HIGH integrity and lose
# drag-and-drop into every Medium app. Refuse rather than silently ship that.
$uiManifest = [System.Text.Encoding]::ASCII.GetString([System.IO.File]::ReadAllBytes($srcBuilt))
if ($uiManifest -match 'uiAccess="true"') {
    Log "ERROR: $srcBuilt has uiAccess=true. The UI must stay Medium or drag-out breaks."
    Log "       Build it with a plain: cargo build --release"
    exit 1
}
Log "UI manifest: uiAccess=false confirmed (Medium; drag-out intact)"

# The broker is the piece that DOES need uiAccess. Without it this install is just
# a copy of the portable build, so say so loudly rather than appearing to work.
$srcBroker = Join-Path $root 'trontsnap-hotkeys.exe'
if (-not (Test-Path $srcBroker)) { $srcBroker = Join-Path $root 'target\release\trontsnap-hotkeys.exe' }
if (-not (Test-Path $srcBroker)) {
    Log "ERROR: trontsnap-hotkeys.exe not found. It is what makes bare PrtSc work over"
    Log "       elevated windows. Build it with: cargo build --release -p trontsnap-hotkeys"
    exit 1
}
$brokerManifest = [System.Text.Encoding]::ASCII.GetString([System.IO.File]::ReadAllBytes($srcBroker))
if ($brokerManifest -notmatch 'uiAccess="true"') {
    Log "ERROR: $srcBroker lacks uiAccess=true; it would be pointless."
    exit 1
}
Log "broker manifest: uiAccess=true confirmed"

# --- 2. trusted signing cert (reuse TrontEQ's; else make a machine-local one) ----
$pfxPass = 'tronteq'
$pfx = 'C:\ProgramData\TrontEq\dev-cert.pfx'
if (Test-Path $pfx) {
    Log "signing cert: reusing $pfx (already trusted in LocalMachine\Root)"
} else {
    $pfx = Join-Path $dataDir 'dev-cert.pfx'
    if (-not (Test-Path $pfx)) {
        Log "signing cert: generating a machine-local self-signed code-signing cert"
        $cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject 'CN=TrontSnap Dev' `
            -KeyUsage DigitalSignature -FriendlyName 'TrontSnap Dev' -CertStoreLocation 'Cert:\CurrentUser\My'
        $sec = ConvertTo-SecureString $pfxPass -Force -AsPlainText
        Export-PfxCertificate -Cert $cert -FilePath $pfx -Password $sec | Out-Null
        $cer = Join-Path $dataDir 'dev-cert.cer'
        Export-Certificate -Cert $cert -FilePath $cer | Out-Null
        Import-Certificate -FilePath $cer -CertStoreLocation 'Cert:\LocalMachine\Root' | Out-Null
        Import-Certificate -FilePath $cer -CertStoreLocation 'Cert:\LocalMachine\TrustedPublisher' | Out-Null
        Log "  generated + trusted $pfx"
    }
}

# --- 3. stop any running instance ------------------------------------------------
& (Join-Path $sys32 'taskkill.exe') /IM trontsnap.exe /F 2>&1 | Out-Null
$deadline = (Get-Date).AddSeconds(6)
while ((Get-Process trontsnap -ErrorAction SilentlyContinue) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 200 }
if (Get-Process trontsnap -ErrorAction SilentlyContinue) { Log "WARN: a trontsnap.exe is still running; the copy below may fail" }

# NOTE: removing the portable exe happens LAST, in step 7, deliberately. It used
# to happen here, before the install was proven, so any later failure (copy
# refused, signtool missing, signature not Valid) left the machine with the
# portable exe deleted, the Run key still aimed at it, and an unsigned Program
# Files copy Windows would refuse to grant uiAccess. Nothing bootable, no way
# back. Do not move it earlier.

# --- 4. install BOTH binaries into Program Files ---------------------------------
# The UI finds the broker by looking for it next to itself, so they must land in
# the same directory.
New-Item -ItemType Directory -Force -Path $installDir | Out-Null
Copy-Item $srcBuilt $destExe -Force
# Test-Path is NOT a copy verification: on a re-install the destination already
# exists, so a failed copy (the running-exe case warned about above) still passes
# it, and a STALE binary then gets signed and reported as freshly installed.
# Compare content.
if ((Get-FileHash $srcBuilt).Hash -ne (Get-FileHash $destExe -ErrorAction SilentlyContinue).Hash) {
    Log "ERROR: $destExe does not match the source after copy (locked / access denied)"
    exit 1
}
Log "installed -> $destExe ($((Get-Item $destExe).Length) bytes)"

& (Join-Path $sys32 'taskkill.exe') /IM trontsnap-hotkeys.exe /F 2>&1 | Out-Null
Start-Sleep -Milliseconds 300
$destBroker = Join-Path $installDir 'trontsnap-hotkeys.exe'
Copy-Item $srcBroker $destBroker -Force
if ((Get-FileHash $srcBroker).Hash -ne (Get-FileHash $destBroker -ErrorAction SilentlyContinue).Hash) {
    Log "ERROR: $destBroker does not match the source after copy (locked / access denied)"
    exit 1
}
Log "installed -> $destBroker ($((Get-Item $destBroker).Length) bytes)"

# --- 5. sign the installed exe (uiAccess is denied without a valid signature) -----
$signtool = 'C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\signtool.exe'
if (-not (Test-Path $signtool)) { $signtool = 'C:\Program Files (x86)\Windows Kits\10\bin\x64\signtool.exe' }
if (-not (Test-Path $signtool)) {
    $signtool = (Get-ChildItem 'C:\Program Files (x86)\Windows Kits\10\bin' -Recurse -Filter signtool.exe -ErrorAction SilentlyContinue |
                 Where-Object { $_.FullName -match '\\x64\\' } | Select-Object -First 1 -ExpandProperty FullName)
}
if (-not $signtool -or -not (Test-Path $signtool)) { Log "ERROR: signtool.exe not found (install the Windows SDK)"; exit 1 }
# Only the BROKER strictly needs a signature (uiAccess is denied without one),
# but sign both: a signed UI is one less SmartScreen prompt and costs nothing.
# /tr + /td add a trusted TIMESTAMP. Without it, every installed binary stops
# verifying the moment the signing cert expires (2027-04-17 for the current dev
# cert), Windows silently denies the broker uiAccess, and bare PrtSc dies on an
# install nobody touched. A timestamped signature stays valid past expiry.
# The timestamp server is best-effort: offline machines still get a signature,
# just one with an expiry date, so a failure here is logged rather than fatal.
$signArgs = @('sign','/v','/fd','SHA256','/f',$pfx,'/p',$pfxPass,'/tr','http://timestamp.digicert.com','/td','SHA256')
foreach ($target in @($destExe, $destBroker)) {
    & $signtool @signArgs $target 2>&1 | ForEach-Object { Log "  $_" }
    if ($LASTEXITCODE -ne 0) {
        Log "  timestamping failed (offline?), retrying without a timestamp"
        & $signtool sign /v /fd SHA256 /f $pfx /p $pfxPass $target 2>&1 | ForEach-Object { Log "  $_" }
        if ($LASTEXITCODE -ne 0) { Log "ERROR: signtool failed for $target (exit $LASTEXITCODE)"; exit 1 }
    }
}
# Gate on BOTH, not just the broker. An unsigned UI is not fatal to uiAccess but
# it is a SmartScreen prompt and a sign that something went wrong above.
foreach ($target in @($destBroker, $destExe)) {
    $sig = Get-AuthenticodeSignature $target
    Log "signature: $(Split-Path $target -Leaf) -> $($sig.Status)"
    if ($sig.Status -ne 'Valid') { Log "ERROR: $target is not validly signed"; exit 1 }
}

# --- 6. autostart -> the installed exe (Run key; a Run/shell launch grants uiAccess) ---
#
# NEVER `New-Item -Path <existing key> -Force` HERE. On an EXISTING registry key
# that does not mean "create if missing"; it RECREATES the key and destroys every
# value in it. This script used to do exactly that to both keys below, which
# silently wiped every other application's per-user autostart entry, and every
# TrontSnap setting (theme, gradient, custom hotkey binds), on every single
# install. Proven and repaired 2026-07-27. Create only when genuinely absent.
$run = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$cmd = "`"$destExe`" --startup"
if (-not (Test-Path $run)) { New-Item -Path $run -Force | Out-Null }
Set-ItemProperty -Path $run -Name 'TrontSnap' -Value $cmd
$appKey = 'HKCU:\Software\TrontSnap'
if (-not (Test-Path $appKey)) { New-Item -Path $appKey -Force | Out-Null }
# -Force on New-ItemProperty is fine: it overwrites one VALUE, not the key.
New-ItemProperty -Path $appKey -Name 'AutostartInit' -Value 1 -PropertyType DWord -Force | Out-Null
Log "autostart -> $cmd"

# --- 7. NOW remove the portable exe, once the install is proven good -------------
# Everything above has succeeded: both binaries copied, the broker's signature
# verified Valid, autostart repointed. Only now is it safe to take away the thing
# the user would otherwise fall back to.
#
# Leaving it would mean two TrontSnaps racing for the same hotkey registration,
# where the first to register wins and the loser is silently dead.
#
# DELETE THE FILE, NOT THE FOLDER. That folder is also the THUMBNAIL CACHE:
# thumbs.rs resolves it via dirs::cache_dir(), i.e. %LOCALAPPDATA%\TrontSnap\thumbs.
# Wiping the directory throws away every cached thumbnail (~17.6k screenshots on
# this machine) and forces a full re-decode on the next gallery scroll. Doing
# exactly that is how this was found.
#
# Screenshots live in Pictures\TrontSnap and settings in HKCU; neither is touched.
$portableExe = Join-Path $PortableDir 'trontsnap.exe'
if (Test-Path $portableExe) {
    $keep = Join-Path $PortableDir 'trontsnap.log'
    if (Test-Path $keep) { Copy-Item $keep (Join-Path $dataDir 'portable-last.log') -Force }
    Remove-Item $portableExe -Force -ErrorAction SilentlyContinue
    if (Test-Path $portableExe) {
        Log "WARN: could not remove $portableExe (still running?); delete it by hand"
    } else {
        Log "removed portable exe -> $portableExe (thumbnail cache left intact)"
    }
} else {
    Log "portable exe: none at $portableExe (nothing to clean up)"
}

Log "RESULT: installed + signed OK. Launch it at Medium ('Install/Launch TrontSnap.cmd') so uiAccess is granted."
