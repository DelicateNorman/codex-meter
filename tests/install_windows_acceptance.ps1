param(
    [Parameter(Mandatory = $true)][string]$AssetDir,
    [Parameter(Mandatory = $true)][string]$AssetName,
    [Parameter(Mandatory = $true)][string]$OldBinary
)

$ErrorActionPreference = "Stop"
$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$testRoot = Join-Path $env:RUNNER_TEMP ("codex-meter-installer-" + [guid]::NewGuid().ToString("N"))
$binDir = Join-Path $testRoot "bin"
$historyHome = Join-Path $testRoot "history"
$sessions = Join-Path $testRoot "sessions"
$osHome = Join-Path $testRoot "os-home"
$installer = [scriptblock]::Create((Get-Content -Raw (Join-Path $repositoryRoot "install.ps1")))

try {
    New-Item -ItemType Directory -Force -Path $binDir, $osHome | Out-Null
    $destination = Join-Path $binDir "codex-meter.exe"
    Copy-Item $OldBinary $destination

    python (Join-Path $repositoryRoot "tests/release_history_guard.py") seed `
        --binary $destination --home $historyHome --sessions $sessions
    python (Join-Path $repositoryRoot "tests/release_history_guard.py") manifest `
        --home $historyHome --output (Join-Path $testRoot "history-before.json")

    $oldHash = (Get-FileHash -Algorithm SHA256 $destination).Hash
    $newHash = (Get-FileHash -Algorithm SHA256 (Join-Path $AssetDir $AssetName)).Hash
    $env:CODEX_METER_HOME = $historyHome
    & $installer -BaseUrl $AssetDir -BinDir $binDir
    if ((Get-FileHash -Algorithm SHA256 $destination).Hash -ne $newHash) { throw "new binary hash mismatch" }
    if ((Get-FileHash -Algorithm SHA256 "$destination.previous").Hash -ne $oldHash) { throw "backup hash mismatch" }
    python (Join-Path $repositoryRoot "tests/release_history_guard.py") manifest `
        --home $historyHome --expect (Join-Path $testRoot "history-before.json")

    $tampered = Join-Path $testRoot "tampered"
    New-Item -ItemType Directory $tampered | Out-Null
    Copy-Item (Join-Path $AssetDir $AssetName) (Join-Path $tampered $AssetName)
    Copy-Item (Join-Path $AssetDir "SHA256SUMS") (Join-Path $tampered "SHA256SUMS")
    [System.IO.File]::AppendAllText((Join-Path $tampered $AssetName), "x")
    $failed = $false
    try {
        & $installer -BaseUrl $tampered -BinDir $binDir
    }
    catch { $failed = $true }
    if (-not $failed) { throw "tampered release unexpectedly installed" }
    if ((Get-FileHash -Algorithm SHA256 $destination).Hash -ne $newHash) { throw "failed checksum changed installation" }
    python (Join-Path $repositoryRoot "tests/release_history_guard.py") manifest `
        --home $historyHome --expect (Join-Path $testRoot "history-before.json")

    & $installer -BinDir $binDir -Rollback
    if ((Get-FileHash -Algorithm SHA256 $destination).Hash -ne $oldHash) { throw "rollback did not restore old binary" }
    if ((Get-FileHash -Algorithm SHA256 "$destination.previous").Hash -ne $newHash) { throw "rollback did not retain new binary" }
    python (Join-Path $repositoryRoot "tests/release_history_guard.py") manifest `
        --home $historyHome --expect (Join-Path $testRoot "history-before.json")

    & $installer -BaseUrl $AssetDir -BinDir $binDir | Out-Null
    python (Join-Path $repositoryRoot "tests/release_history_guard.py") database `
        --home $historyHome --output (Join-Path $testRoot "database-before.json")
    & $destination --home $historyHome --no-color summary --period all | Out-Null
    & $destination --home $historyHome --no-color history --group month | Out-Null
    & $destination --home $historyHome export --format json | Out-Null
    & $destination --home $historyHome doctor | Out-Null
    python (Join-Path $repositoryRoot "tests/release_history_guard.py") database `
        --home $historyHome --expect (Join-Path $testRoot "database-before.json")

    Write-Host "Windows installer upgrade/checksum/rollback/history acceptance passed for $AssetName"
}
finally {
    Remove-Item Env:CODEX_METER_HOME -ErrorAction SilentlyContinue
    if (Test-Path $testRoot) { Remove-Item -Recurse -Force $testRoot }
}
