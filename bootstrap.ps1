# TrontSnap installer. Signs trontsnap.exe and installs it into %ProgramFiles%\TrontSnap
# so Windows will grant it uiAccess (a signed exe in a secure location). uiAccess lets
# TrontSnap's keyboard hook see keystrokes over ELEVATED windows (TrontEQ, Task Manager,
# elevated terminals) WITHOUT elevating TrontSnap itself, so it stays Medium integrity and
# its drag-out keeps working.
#
# Single-phase, elevated, $PSScriptRoot-relative, idempotent. No test-signing, no reboot
# (those were only for TrontEQ's kernel-loaded APO DLL; TrontSnap is a plain user-mode exe
# that just needs a valid Authenticode signature chaining to a trusted root).
#
# Run via "Install TrontSnap.cmd" (it self-elevates and then launches the app at Medium so
# AppInfo actually grants uiAccess). Running bootstrap alone installs but does NOT launch.

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
$prebuilt = Join-Path $root 'trontsnap.exe'
$srcExe   = Join-Path $root 'target\release\trontsnap.exe'
if (Test-Path $prebuilt) {
    $srcBuilt = $prebuilt
    Log "layout: prebuilt ($prebuilt)"
} else {
    if (-not (Test-Path $srcExe)) {
        Log "building: cargo build --release --features uiaccess"
        Push-Location $root
        try { & cargo build --release --features uiaccess 2>&1 | ForEach-Object { Log "  $_" } }
        finally { Pop-Location }
    }
    if (-not (Test-Path $srcExe)) { Log "ERROR: $srcExe not found and the build did not produce it"; exit 1 }
    $srcBuilt = $srcExe
    Log "layout: source ($srcExe)"
}

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

# --- 3. stop any running instance, then install into Program Files ---------------
& (Join-Path $sys32 'taskkill.exe') /IM trontsnap.exe /F 2>&1 | Out-Null
$deadline = (Get-Date).AddSeconds(6)
while ((Get-Process trontsnap -ErrorAction SilentlyContinue) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 200 }
New-Item -ItemType Directory -Force -Path $installDir | Out-Null
Copy-Item $srcBuilt $destExe -Force
if (-not (Test-Path $destExe)) { Log "ERROR: could not copy exe to $destExe (need admin / file locked)"; exit 1 }
Log "installed -> $destExe ($((Get-Item $destExe).Length) bytes)"

# --- 4. sign the installed exe (uiAccess is denied without a valid signature) -----
$signtool = 'C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\signtool.exe'
if (-not (Test-Path $signtool)) { $signtool = 'C:\Program Files (x86)\Windows Kits\10\bin\x64\signtool.exe' }
if (-not (Test-Path $signtool)) { Log "ERROR: signtool.exe not found (install the Windows SDK)"; exit 1 }
& $signtool sign /v /fd SHA256 /f $pfx /p $pfxPass $destExe 2>&1 | ForEach-Object { Log "  $_" }
$sig = Get-AuthenticodeSignature $destExe
Log "signature: $($sig.Status)"
if ($sig.Status -ne 'Valid') { Log "ERROR: installed exe is not validly signed -> Windows will deny uiAccess"; exit 1 }

# --- 5. autostart -> the installed exe (Run key; a Run/shell launch grants uiAccess) ---
$run = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$cmd = "`"$destExe`" --startup"
New-Item -Path $run -Force | Out-Null
Set-ItemProperty -Path $run -Name 'TrontSnap' -Value $cmd
$appKey = 'HKCU:\Software\TrontSnap'
New-Item -Path $appKey -Force | Out-Null
New-ItemProperty -Path $appKey -Name 'AutostartInit' -Value 1 -PropertyType DWord -Force | Out-Null
Log "autostart -> $cmd"

Log "RESULT: installed + signed OK. Launch it at Medium ('Install/Launch TrontSnap.cmd') so uiAccess is granted."
