param([Parameter(Mandatory=$true)][string]$PortfolioRoot)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$inputDir = Join-Path $repo 'target/showcase-input'
New-Item -ItemType Directory -Path $inputDir -Force | Out-Null
# Only published portfolio images, never personal capture history.
$names = @('terrainlab.jpg','biomass.jpg','campfire.jpg','colony.jpg',
    'buttloads2.jpg','monkeportalsrender.jpg','ballpit.jpg','ai1.png','astarvibes.png',
    '2015-01-10_08-36-05.png','2015-02-03_18-23-19.png','2015-04-18_03-20-02.jpg',
    '2015-05-15_02-29-22.png','2015-06-14_16-34-46.png','2015-07-10_02-16-47.jpg',
    '2015-08-22_01-30-36.png','2015-09-01_18-21-41.png','2014-09-06_04-27-54.png',
    '2014-09-06_18-32-33.png','2014-12-08_14-13-31-550x309.jpg',
    '2014-12-08_14-16-21-550x309.jpg','20238-shot0.png','bomberman1.jpg',
    'bomberman2.jpg','2015-07-10_02-16-47.jpg','2014-12-08_14-16-21-550x309.jpg',
    'ai1.png','ballpit.jpg')
for ($i = 0; $i -lt $names.Count; $i++) {
    $source = Join-Path (Join-Path $PortfolioRoot 'img') $names[$i]
    $dest = Join-Path $inputDir (('{0:D2}-' -f ($i + 1)) + $names[$i])
    Copy-Item -LiteralPath $source -Destination $dest
}
Push-Location $repo
try {
    cargo test --locked -p trontsnap capture_current_app -- --ignored --nocapture
    if ($LASTEXITCODE -ne 0) { throw 'Production UI capture failed' }
} finally { Pop-Location }
