# Cut a TrontSnap release LOCALLY: bump versions, commit, annotated-tag. Never pushes.
# Same behaviour as scripts/release.sh, for the Windows box.
#
# usage:  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\release.ps1 X.Y.Z [-DryRun]
#         (from an open PowerShell: .\scripts\release.ps1 X.Y.Z -DryRun ; '--dry-run' works too)
#
# Refuses (listing every reason it found) unless the working tree is clean, HEAD is on a
# branch, X.Y.Z is greater than the current version, tag vX.Y.Z does not exist, today's
# versions agree everywhere, and CHANGELOG.md already has a non-empty '## vX.Y.Z' section
# (committed; this script never invents release notes).
#
# Then it edits Cargo.toml, hotkeyd\Cargo.toml and the 'trontsnap' / 'trontsnap-hotkeys'
# entries of Cargo.lock by TEXT (no cargo call), commits those 3 files as
# "Release TrontSnap X.Y.Z" and tags vX.Y.Z. -DryRun = same checks and edits in memory,
# shows what would change, writes nothing.
#
# Constraints this file lives under:
#   - Windows PowerShell 5.1: no ?: ?? && || ternary/pipeline-chain, no pwsh-only syntax.
#   - ASCII only: 5.1 reads a BOM-less script as ANSI, so one smart quote would corrupt it.
#   - The files are edited as Latin-1 text. That maps bytes 1:1 to chars, so the UTF-8 BOM
#     on both Cargo.toml files and whatever line endings autocrlf produced survive
#     untouched. (Set-Content/Get-Content would re-encode them.)
#   - .NET file calls resolve relative paths against the PROCESS directory, not the
#     PowerShell location, so every path below is absolute.

param(
    [Parameter(Position = 0)][string]$Version,
    [switch]$DryRun,
    [Parameter(ValueFromRemainingArguments = $true)][string[]]$Rest
)

# 'Continue', not 'Stop': in 5.1 a native tool's stderr line becomes a terminating error
# under 'Stop' (same trap bootstrap.ps1 documents). Every git call is checked by hand.
$ErrorActionPreference = 'Continue'

$Root   = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$Latin1 = [System.Text.Encoding]::GetEncoding(28591)
$Semver = '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'

function Fail([string]$Msg, [int]$Code) {
    Write-Host $Msg -ForegroundColor Red
    exit $Code
}

function Read-Raw([string]$Path) { return $Latin1.GetString([System.IO.File]::ReadAllBytes($Path)) }
function Write-Raw([string]$Path, [string]$Text) { [System.IO.File]::WriteAllBytes($Path, $Latin1.GetBytes($Text)) }

# === Argument parsing ===
$New = ''
$all = @()
if ($Version) { $all += $Version }
if ($Rest)    { $all += $Rest }
foreach ($a in $all) {
    if ($a -ieq '--dry-run' -or $a -ieq '-dry-run') { $DryRun = [switch]::Present; continue }
    if ($a.StartsWith('-')) { Fail "unknown option: $a" 2 }
    if ($New -ne '') { Fail "one version only (got '$New' and '$a')" 2 }
    $New = $a
}
if ($New.StartsWith('v')) { $New = $New.Substring(1) }
if ($New -eq '') { Fail 'usage: release.ps1 X.Y.Z [-DryRun]' 2 }
if ($New -notmatch $Semver) { Fail "version must be plain X.Y.Z (got '$New')" 2 }
$Tag = "v$New"

# === Readers (same cuts as scripts/release.sh and .github/scripts/verify-release.sh) ===
# Index range of the [package] table body, or $null. HARD-WON: the header regex allows the
# UTF-8 BOM (as Latin-1 chars) before '[package]' because hotkeyd's header is line 1.
# '\r?$' because .NET's multiline $ stops before '\n' only, so a CRLF file would never match.
function Get-PackageBody([string]$Text) {
    $hdr = [regex]::Match($Text, '(?m)^(\xEF\xBB\xBF)?\[package\][ \t]*(#[^\r\n]*)?\r?$')
    if (-not $hdr.Success) { return $null }
    $start = $hdr.Index + $hdr.Length
    $tail  = $Text.Substring($start)
    $next  = [regex]::Match($tail, '(?m)^\[')
    $len   = $tail.Length
    if ($next.Success) { $len = $next.Index }
    return @($start, $len)
}

# Match of the version value inside [package]: returns absolute index + length, or $null.
function Find-TomlVersion([string]$Text) {
    $body = Get-PackageBody $Text
    if ($null -eq $body) { return $null }
    $seg = $Text.Substring($body[0], $body[1])
    $m = [regex]::Match($seg, '(?m)^version[ \t]*=[ \t]*"(?<v>[^"]*)"')
    if (-not $m.Success) { return $null }
    $g = $m.Groups['v']
    return @(($body[0] + $g.Index), $g.Length)
}

function Get-TomlVersion([string]$Text) {
    $loc = Find-TomlVersion $Text
    if ($null -eq $loc) { return '' }
    return $Text.Substring($loc[0], $loc[1])
}

function Set-TomlVersion([string]$Text, [string]$To) {
    $loc = Find-TomlVersion $Text
    if ($null -eq $loc) { return $null }
    return $Text.Substring(0, $loc[0]) + $To + $Text.Substring($loc[0] + $loc[1])
}

# Cargo.lock writes `name` then `version` on adjacent lines. Closing quote in the pattern
# keeps 'trontsnap' from matching 'trontsnap-hotkeys'.
function Find-LockVersion([string]$Text, [string]$Name) {
    $pat = '(?m)^name = "' + [regex]::Escape($Name) + '"\r?\nversion = "(?<v>[^"]*)"'
    $ms = [regex]::Matches($Text, $pat)
    if ($ms.Count -ne 1) { return $null }
    $g = $ms[0].Groups['v']
    return @($g.Index, $g.Length)
}

function Get-LockVersion([string]$Text, [string]$Name) {
    $loc = Find-LockVersion $Text $Name
    if ($null -eq $loc) { return '' }
    return $Text.Substring($loc[0], $loc[1])
}

function Set-LockVersion([string]$Text, [string]$Name, [string]$To) {
    $loc = Find-LockVersion $Text $Name
    if ($null -eq $loc) { return $null }
    return $Text.Substring(0, $loc[0]) + $To + $Text.Substring($loc[0] + $loc[1])
}

# Body of '## vX.Y.Z' up to the next '## ', blank edges trimmed. $null = no such heading.
function Get-ChangelogSection([string]$Text, [string]$TagName) {
    $head  = '## ' + $TagName
    $found = $false
    $body  = @()
    foreach ($ln in ($Text -split '\r?\n')) {
        if ($ln.StartsWith('## ', [StringComparison]::Ordinal)) {
            if ($found) { break }
            if ($ln -ceq $head -or $ln.StartsWith($head + ' ', [StringComparison]::Ordinal)) { $found = $true }
            continue
        }
        if ($found) { $body += $ln }
    }
    if (-not $found) { return $null }
    $s = 0
    $e = $body.Count - 1
    while ($s -le $e -and $body[$s].Trim() -eq '') { $s++ }
    while ($e -ge $s -and $body[$e].Trim() -eq '') { $e-- }
    if ($e -lt $s) { return '' }
    return (($body[$s..$e]) -join "`n")
}

# Number of lines that differ index-by-index; -1 when the line counts differ.
function Count-ChangedLines([string]$A, [string]$B) {
    $la = $A -split '\n'
    $lb = $B -split '\n'
    if ($la.Count -ne $lb.Count) { return -1 }
    $n = 0
    for ($i = 0; $i -lt $la.Count; $i++) { if ($la[$i] -cne $lb[$i]) { $n++ } }
    return $n
}

function Show-Changes([string]$Label, [string]$A, [string]$B) {
    $la = $A -split '\n'
    $lb = $B -split '\n'
    for ($i = 0; $i -lt $la.Count; $i++) {
        if ($la[$i] -cne $lb[$i]) {
            Write-Host ("  {0}  line {1}" -f $Label, ($i + 1))
            Write-Host ("      - {0}" -f $la[$i].TrimEnd("`r")) -ForegroundColor DarkRed
            Write-Host ("      + {0}" -f $lb[$i].TrimEnd("`r")) -ForegroundColor DarkGreen
        }
    }
}

# === Preflight: collect every problem, refuse once ===
$problems = New-Object System.Collections.ArrayList
function Add-Problem([string]$Msg) { [void]$problems.Add($Msg) }

Set-Location $Root
$null = & git rev-parse --git-dir 2>$null
if ($LASTEXITCODE -ne 0) { Fail 'not inside a git repository' 1 }

$relFiles = @('Cargo.toml', 'hotkeyd\Cargo.toml', 'Cargo.lock', 'CHANGELOG.md')
foreach ($f in $relFiles) {
    if (-not (Test-Path -LiteralPath (Join-Path $Root $f))) { Fail "$f not found (run from a TrontSnap checkout)" 1 }
}

$branch = & git symbolic-ref --short -q HEAD 2>$null
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrEmpty($branch)) {
    Add-Problem 'HEAD is detached; check out the branch you are releasing from'
    $branch = ''
}

# Untracked files count as dirty on purpose: a forgotten new source file is how
# "builds here, fails in CI" happens.
$dirty = @(& git status --porcelain 2>$null)
if ($dirty.Count -gt 0) {
    $shown = ($dirty | Select-Object -First 8 | ForEach-Object { '        ' + $_ }) -join "`n"
    Add-Problem ("working tree is not clean ({0} path(s)); commit or stash first:`n{1}" -f $dirty.Count, $shown)
}

$null = & git var GIT_COMMITTER_IDENT 2>$null
if ($LASTEXITCODE -ne 0) {
    Add-Problem 'no git identity (git config user.name / user.email); the commit and tag would fail half-way'
}

$null = & git rev-parse -q --verify "refs/tags/$Tag" 2>$null
if ($LASTEXITCODE -eq 0) { Add-Problem "tag $Tag already exists locally" }

$txtRoot = Read-Raw (Join-Path $Root 'Cargo.toml')
$txtHk   = Read-Raw (Join-Path $Root 'hotkeyd\Cargo.toml')
$txtLock = Read-Raw (Join-Path $Root 'Cargo.lock')
$txtLog  = Read-Raw (Join-Path $Root 'CHANGELOG.md')

$cur  = Get-TomlVersion $txtRoot
$vHk  = Get-TomlVersion $txtHk
$vLt  = Get-LockVersion $txtLock 'trontsnap'
$vLh  = Get-LockVersion $txtLock 'trontsnap-hotkeys'
if ($cur -ne $vHk -or $cur -ne $vLt -or $cur -ne $vLh) {
    Add-Problem ("versions disagree today (a half-finished bump?): Cargo.toml='{0}' hotkeyd/Cargo.toml='{1}' Cargo.lock trontsnap='{2}' trontsnap-hotkeys='{3}'" -f $cur, $vHk, $vLt, $vLh)
}
if ($cur -notmatch $Semver) {
    Add-Problem "current version '$cur' is not plain X.Y.Z, so 'greater than' is undefined"
} elseif (-not ([version]$New -gt [version]$cur)) {
    Add-Problem "$New is not greater than the current version $cur"
}

$notes = Get-ChangelogSection $txtLog $Tag
if ($null -eq $notes) {
    Add-Problem "CHANGELOG.md has no '## $Tag' section; write it (and commit it) first"
} elseif ($notes -eq '') {
    Add-Problem "CHANGELOG.md has a '## $Tag' heading but the section is empty"
}

if ($problems.Count -gt 0) {
    Write-Host ("release.ps1: refusing to release {0} ({1} problem(s)):" -f $New, $problems.Count) -ForegroundColor Red
    foreach ($p in $problems) { Write-Host ("  - {0}" -f $p) -ForegroundColor Red }
    Write-Host 'Nothing was changed.' -ForegroundColor Red
    exit 1
}

# === Edit in memory, prove it, only then touch the tree ===
$newRoot = Set-TomlVersion $txtRoot $New
$newHk   = Set-TomlVersion $txtHk $New
$newLock = Set-LockVersion $txtLock 'trontsnap' $New
if ($null -ne $newLock) { $newLock = Set-LockVersion $newLock 'trontsnap-hotkeys' $New }

$bad = New-Object System.Collections.ArrayList
if ($null -eq $newRoot -or $null -eq $newHk -or $null -eq $newLock) {
    [void]$bad.Add('could not locate a version field to edit (file layout drifted)')
} else {
    if ((Get-TomlVersion $newRoot) -ne $New) { [void]$bad.Add("Cargo.toml did not take $New") }
    if ((Get-TomlVersion $newHk)   -ne $New) { [void]$bad.Add("hotkeyd\Cargo.toml did not take $New") }
    if ((Get-LockVersion $newLock 'trontsnap')         -ne $New) { [void]$bad.Add("Cargo.lock trontsnap did not take $New") }
    if ((Get-LockVersion $newLock 'trontsnap-hotkeys') -ne $New) { [void]$bad.Add("Cargo.lock trontsnap-hotkeys did not take $New") }
    # Exactly the intended lines changed, nothing else (BOM and line endings included).
    if ((Count-ChangedLines $txtRoot $newRoot) -ne 1) { [void]$bad.Add('Cargo.toml: expected exactly 1 changed line') }
    if ((Count-ChangedLines $txtHk $newHk)     -ne 1) { [void]$bad.Add('hotkeyd\Cargo.toml: expected exactly 1 changed line') }
    if ((Count-ChangedLines $txtLock $newLock) -ne 2) { [void]$bad.Add('Cargo.lock: expected exactly 2 changed lines') }
    if ($newRoot.Substring(0, 3) -cne $txtRoot.Substring(0, 3)) { [void]$bad.Add('Cargo.toml BOM/first bytes changed') }
    if ($newHk.Substring(0, 3)   -cne $txtHk.Substring(0, 3))   { [void]$bad.Add('hotkeyd\Cargo.toml BOM/first bytes changed') }
}
if ($bad.Count -gt 0) {
    Write-Host 'release.ps1: the version edit did not come out right; nothing was changed:' -ForegroundColor Red
    foreach ($p in $bad) { Write-Host ("  - {0}" -f $p) -ForegroundColor Red }
    exit 1
}

$remote = & git config --get "branch.$branch.remote" 2>$null
if ([string]::IsNullOrEmpty($remote)) { $remote = 'origin' }
$push = "git push --atomic $remote $branch $Tag"

Write-Host ("== TrontSnap release {0} -> {1} (tag {2} on {3}) ==" -f $cur, $New, $Tag, $branch)
Write-Host ''
Show-Changes 'Cargo.toml' $txtRoot $newRoot
Show-Changes 'hotkeyd\Cargo.toml' $txtHk $newHk
Show-Changes 'Cargo.lock' $txtLock $newLock
Write-Host ''
$noteLines = $notes -split "`n"
Write-Host ("CHANGELOG '## {0}' section ({1} lines), becomes the draft release notes:" -f $Tag, $noteLines.Count)
$noteLines | Select-Object -First 4 | ForEach-Object { Write-Host ('    ' + $_) }
Write-Host ''

if ($DryRun) {
    Write-Host ("DRY RUN: nothing written. A real run would commit ""Release TrontSnap {0}"" and tag {1}." -f $New, $Tag)
} else {
    $commitFiles = @('Cargo.toml', 'hotkeyd/Cargo.toml', 'Cargo.lock')
    try {
        Write-Raw (Join-Path $Root 'Cargo.toml') $newRoot
        Write-Raw (Join-Path $Root 'hotkeyd\Cargo.toml') $newHk
        Write-Raw (Join-Path $Root 'Cargo.lock') $newLock
    } catch {
        # Tree was clean at the start, so checkout puts back exactly what we touched.
        & git checkout -- $commitFiles
        Fail ("release.ps1: could not write the version files ({0}); restored, nothing committed" -f $_.Exception.Message) 1
    }
    # Pathspec form: commits only these files even if something else is staged.
    & git commit -q -m "Release TrontSnap $New" -- $commitFiles
    if ($LASTEXITCODE -ne 0) {
        & git checkout -- $commitFiles
        Fail 'release.ps1: git commit failed (a hook?); version files restored, nothing committed' 1
    }
    & git tag -a $Tag -m "TrontSnap $New"
    if ($LASTEXITCODE -ne 0) {
        Fail "release.ps1: the commit landed but tagging failed. Tag it by hand: git tag -a $Tag -m ""TrontSnap $New""" 1
    }
    & git --no-pager show --stat --format='commit %h  %s' HEAD
    Write-Host ''
    & git --no-pager tag -n1 -l $Tag
    Write-Host ''
    Write-Host 'Committed and tagged LOCALLY. Nothing has been pushed.'
}

Write-Host ''
Write-Host 'Next, push the branch and the tag together (this is what starts CI):'
Write-Host ''
Write-Host "    $push"
Write-Host ''
Write-Host 'Then CI (.github/workflows/release.yml) will:'
Write-Host "  1. verify   tag == Cargo.toml == hotkeyd == Cargo.lock; CHANGELOG '## $Tag' -> notes"
Write-Host "  2. windows  build --locked; TrontSnap.exe, TrontSnap-v$New-windows.zip (unsigned bundle), Rajdhani-OFL.txt"
Write-Host '  3. linux    ubuntu-22.04 build; AppImage + .zsync, .deb, tarball'
Write-Host "  4. publish  SHA256SUMS + a DRAFT release titled 'TrontSnap $New'"
Write-Host "You then set the title ('TrontSnap $New`: <headline>') in the GitHub UI and publish."
Write-Host 'To rehearse the builds first: push the branch only, then Actions > Release > Run workflow'
Write-Host "(branch, tag input $Tag, dry_run ticked). See RELEASING.md."
exit 0
