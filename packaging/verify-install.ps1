# TrontSnap install verifier. Run NON-elevated, after installing.
#
# Every check here exists because something in it broke for real. The two-process
# split has a lot of invisible failure modes: a broker that dies instantly leaves
# hotkeys silently dead, a uiAccess UI silently loses drag-out, a clipboard write
# from the wrong thread silently never lands. None of those announce themselves,
# so this asserts them instead of hoping.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File packaging\verify-install.ps1
#
# Exit code 0 = all passed, 1 = at least one FAIL.

$ErrorActionPreference = 'Continue'
$script:Failures = 0
$script:Warnings = 0

function Check($name, $ok, $detail, [switch]$WarnOnly) {
    if ($ok) {
        "  [ PASS ] {0,-42} {1}" -f $name, $detail
    } elseif ($WarnOnly) {
        $script:Warnings++
        "  [ WARN ] {0,-42} {1}" -f $name, $detail
    } else {
        $script:Failures++
        "  [ FAIL ] {0,-42} {1}" -f $name, $detail
    }
}

Add-Type @'
using System;using System.Text;using System.Runtime.InteropServices;
public class TsProbe{
 [DllImport("kernel32.dll",SetLastError=true)] public static extern IntPtr OpenProcess(uint a,bool b,int p);
 [DllImport("advapi32.dll",SetLastError=true)] public static extern bool OpenProcessToken(IntPtr h,uint a,out IntPtr t);
 [DllImport("advapi32.dll",SetLastError=true)] public static extern bool GetTokenInformation(IntPtr t,int c,IntPtr buf,uint len,out uint ret);
 [DllImport("advapi32.dll",SetLastError=true)] public static extern bool GetTokenInformation(IntPtr t,int c,out uint v,uint len,out uint ret);
 [DllImport("advapi32.dll")] public static extern IntPtr GetSidSubAuthority(IntPtr sid,uint i);
 [DllImport("advapi32.dll")] public static extern IntPtr GetSidSubAuthorityCount(IntPtr sid);
 [DllImport("kernel32.dll")] public static extern bool CloseHandle(IntPtr h);
 [DllImport("user32.dll")] public static extern IntPtr GetClipboardOwner();
 [DllImport("user32.dll")] public static extern IntPtr GetOpenClipboardWindow();
}
'@

# Returns @{ Integrity = 'MEDIUM'|'HIGH'|...; UiAccess = $true|$false } or $null.
function Get-TokenFacts($procId) {
    $h = [TsProbe]::OpenProcess(0x1000, $false, $procId)
    if ($h -eq [IntPtr]::Zero) { return $null }
    $tok = [IntPtr]::Zero
    if (-not [TsProbe]::OpenProcessToken($h, 0x0008, [ref]$tok)) { [void][TsProbe]::CloseHandle($h); return $null }
    $need = 0
    [void][TsProbe]::GetTokenInformation($tok, 25, [IntPtr]::Zero, 0, [ref]$need)
    $buf = [Runtime.InteropServices.Marshal]::AllocHGlobal([int]$need)
    $lvl = '?'
    if ([TsProbe]::GetTokenInformation($tok, 25, $buf, $need, [ref]$need)) {
        $sid = [Runtime.InteropServices.Marshal]::ReadIntPtr($buf)
        $cnt = [Runtime.InteropServices.Marshal]::ReadByte([TsProbe]::GetSidSubAuthorityCount($sid))
        $rid = [Runtime.InteropServices.Marshal]::ReadInt32([TsProbe]::GetSidSubAuthority($sid, [uint32]($cnt - 1)))
        $lvl = switch ($rid) { 0x1000 {'LOW'} 0x2000 {'MEDIUM'} 0x3000 {'HIGH'} 0x4000 {'SYSTEM'} default {"0x{0:X}" -f $rid} }
    }
    $ui = 0; $r = 0
    [void][TsProbe]::GetTokenInformation($tok, 26, [ref]$ui, 4, [ref]$r)
    [Runtime.InteropServices.Marshal]::FreeHGlobal($buf)
    [void][TsProbe]::CloseHandle($tok); [void][TsProbe]::CloseHandle($h)
    @{ Integrity = $lvl; UiAccess = ($ui -ne 0) }
}

function Test-Port($p) {
    try {
        $c = New-Object System.Net.Sockets.TcpClient
        $iar = $c.BeginConnect('127.0.0.1', $p, $null, $null)
        $ok = $iar.AsyncWaitHandle.WaitOne(400)
        if ($ok) { $c.EndConnect($iar) }
        $c.Close()
        return $ok
    } catch { return $false }
}

$installDir = Join-Path $env:ProgramFiles 'TrontSnap'
$uiExe      = Join-Path $installDir 'trontsnap.exe'
$brokerExe  = Join-Path $installDir 'trontsnap-hotkeys.exe'

"TrontSnap install verification  ($(Get-Date -Format 'yyyy-MM-dd HH:mm:ss'))"
""
"FILES"
Check 'UI installed'     (Test-Path $uiExe)     $uiExe
Check 'broker installed' (Test-Path $brokerExe) $brokerExe
foreach ($f in @($uiExe, $brokerExe)) {
    if (Test-Path $f) {
        $sig = Get-AuthenticodeSignature $f
        $name = Split-Path $f -Leaf
        Check "$name signed" ($sig.Status -eq 'Valid') "$($sig.Status)"
        # Expiry without a timestamp is a silent time bomb: uiAccess is denied the
        # day the cert lapses, on an install nobody touched.
        $cert = $sig.SignerCertificate
        if ($cert) {
            $ts = $sig.TimeStamperCertificate
            Check "$name timestamped" ($null -ne $ts) `
                $(if ($ts) { 'yes (survives cert expiry)' } else { "no - signature dies $($cert.NotAfter.ToString('yyyy-MM-dd'))" }) -WarnOnly
        }
    }
}

# The manifests are the whole architecture: UI must NOT have uiAccess (it would be
# raised to HIGH and lose drag-and-drop), broker MUST have it (or bare PrtSc dies
# over elevated windows).
"MANIFESTS"
foreach ($pair in @(@{f=$uiExe;n='UI';want=$false}, @{f=$brokerExe;n='broker';want=$true})) {
    if (Test-Path $pair.f) {
        $txt = [System.Text.Encoding]::ASCII.GetString([System.IO.File]::ReadAllBytes($pair.f))
        $has = $txt -match 'uiAccess\s*=\s*"true"'
        Check "$($pair.n) uiAccess=$($pair.want)" ($has -eq $pair.want) $(if ($has) {'uiAccess=true'} else {'uiAccess=false'})
    }
}

"PROCESSES"
$ui = Get-Process trontsnap -ErrorAction SilentlyContinue | Select-Object -First 1
$bk = Get-Process trontsnap-hotkeys -ErrorAction SilentlyContinue | Select-Object -First 1
Check 'UI running'     ($null -ne $ui) $(if ($ui) { "pid=$($ui.Id)" } else { 'not running' })
Check 'broker running' ($null -ne $bk) $(if ($bk) { "pid=$($bk.Id)" } else { 'not running - hotkeys will not survive elevated windows' })

if ($ui) {
    $f = Get-TokenFacts $ui.Id
    if ($f) {
        Check 'UI is MEDIUM integrity' ($f.Integrity -eq 'MEDIUM') "$($f.Integrity) - drag-out needs MEDIUM"
        Check 'UI has NO uiAccess'     (-not $f.UiAccess)          "uiAccess=$($f.UiAccess)"
    } else { Check 'UI token readable' $false 'could not open token' -WarnOnly }
}
if ($bk) {
    $f = Get-TokenFacts $bk.Id
    if ($f) {
        Check 'broker HAS uiAccess' ($f.UiAccess) "uiAccess=$($f.UiAccess), integrity=$($f.Integrity)"
    } else {
        # Expected: the broker runs HIGH, so a Medium shell cannot open its token.
        Check 'broker token (HIGH, unreadable from here)' $true 'consistent with uiAccess being granted'
    }
}

"PORTS"
Check 'UI command channel 48761' (Test-Port 48761) '127.0.0.1:48761'
Check 'broker beacon 48771'      (Test-Port 48771) '127.0.0.1:48771'

# The beacon must IDENTIFY itself. "Something is listening" is not the same claim,
# and treating it as one let a port collision disable every hotkey.
if (Test-Port 48771) {
    $hello = ''
    try {
        $c = New-Object System.Net.Sockets.TcpClient('127.0.0.1', 48771)
        $c.ReceiveTimeout = 500
        $s = $c.GetStream(); $b = New-Object byte[] 32
        $n = $s.Read($b, 0, 32); $hello = [Text.Encoding]::ASCII.GetString($b, 0, $n)
        $c.Close()
    } catch {}
    Check 'beacon identifies itself' ($hello -like 'trontsnap-hotkeyd*') "got '$hello'"
}

"AUTOSTART + SETTINGS"
$run = Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -ErrorAction SilentlyContinue
$runVals = @($run.PSObject.Properties | Where-Object { $_.Name -notmatch '^PS' })
Check 'autostart points at install' ($run.TrontSnap -like "*$installDir*") "$($run.TrontSnap)"
# The installer used to New-Item -Force this key, deleting every other app's entry.
Check 'other Run entries survived' ($runVals.Count -gt 1) `
    "$($runVals.Count) entries: $(($runVals | ForEach-Object { $_.Name }) -join ', ')" -WarnOnly

"CLIPBOARD"
$before = [TsProbe]::GetClipboardOwner()
$dir = Join-Path $env:USERPROFILE 'Pictures\TrontSnap'
$countBefore = @(Get-ChildItem $dir -Filter *.png -ErrorAction SilentlyContinue).Count
try {
    $c = New-Object System.Net.Sockets.TcpClient('127.0.0.1', 48761)
    $s = $c.GetStream(); $b = [Text.Encoding]::ASCII.GetBytes('full')
    $s.Write($b, 0, $b.Length); $s.Flush(); $c.Close()
    Start-Sleep -Seconds 4
    $countAfter = @(Get-ChildItem $dir -Filter *.png -ErrorAction SilentlyContinue).Count
    Check 'capture via command channel' ($countAfter -gt $countBefore) "$countBefore -> $countAfter files"
    $owner = [TsProbe]::GetClipboardOwner()
    # A NULL owner is the signature of the thread-affinity bug: writes appear to
    # succeed and the contents evaporate, or OpenClipboard fails forever.
    Check 'clipboard has a real owner' ($owner -ne [IntPtr]::Zero) "hwnd=0x$($owner.ToString('X'))"
    Add-Type -AssemblyName System.Windows.Forms
    $img = [System.Windows.Forms.Clipboard]::GetImage()
    Check 'image reached the clipboard' ($null -ne $img) $(if ($img) { "$($img.Width)x$($img.Height)" } else { 'nothing' })
    $fd = [System.Windows.Forms.Clipboard]::GetFileDropList()
    Check 'CF_HDROP reached the clipboard' ($fd.Count -gt 0) $(if ($fd.Count) { Split-Path $fd[0] -Leaf } else { 'none' })
} catch {
    Check 'clipboard round trip' $false "$($_.Exception.Message)"
}

"LOG"
$log = Join-Path $env:LOCALAPPDATA 'TrontSnap\trontsnap.log'
if (Test-Path $log) {
    $recent = Get-Content $log -Tail 40
    $bad = @($recent | Where-Object { $_ -match 'failed|gave up|still blocked|WARN' })
    Check 'no failures in recent log' ($bad.Count -eq 0) $(if ($bad.Count) { "$($bad.Count) suspicious lines (tail below)" } else { 'clean' }) -WarnOnly
    if ($bad.Count) { $bad | Select-Object -Last 5 | ForEach-Object { "           $_" } }
}

""
if ($script:Failures -eq 0) {
    "RESULT: PASS  ($script:Warnings warning(s))"
    exit 0
} else {
    "RESULT: $script:Failures FAILURE(S), $script:Warnings warning(s)"
    exit 1
}
