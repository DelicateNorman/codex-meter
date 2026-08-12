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
$legacyBin = Join-Path $testRoot "legacy-python-scripts"
$installer = [scriptblock]::Create((Get-Content -Raw (Join-Path $repositoryRoot "install.ps1")))
$originalUserPath = [Environment]::GetEnvironmentVariable("Path", "User")
$originalProcessPath = $env:Path

try {
    New-Item -ItemType Directory -Force -Path $binDir, $osHome, $legacyBin | Out-Null
    $destination = Join-Path $binDir "codex-meter.exe"
    Copy-Item $OldBinary $destination
    Copy-Item $OldBinary (Join-Path $legacyBin "codex-meter.exe")
    $env:Path = "$legacyBin;$binDir;$originalProcessPath"
    [Environment]::SetEnvironmentVariable(
        "Path", "$legacyBin;$binDir;$originalUserPath", "User"
    )

    python (Join-Path $repositoryRoot "tests/release_history_guard.py") seed `
        --binary $destination --home $historyHome --sessions $sessions
    python (Join-Path $repositoryRoot "tests/release_history_guard.py") manifest `
        --home $historyHome --output (Join-Path $testRoot "history-before.json")

    $oldHash = (Get-FileHash -Algorithm SHA256 $destination).Hash
    $newHash = (Get-FileHash -Algorithm SHA256 (Join-Path $AssetDir $AssetName)).Hash
    $env:CODEX_METER_HOME = $historyHome
    & $installer -BaseUrl $AssetDir -BinDir $binDir
    $resolved = (Get-Command codex-meter -CommandType Application).Source
    if ($resolved -ne $destination) { throw "installer did not put the Rust binary first on PATH: $resolved" }
    if (($env:Path -split ";")[0] -ne $binDir) { throw "process PATH does not start with install directory" }
    if (([Environment]::GetEnvironmentVariable("Path", "User") -split ";")[0] -ne $binDir) {
        throw "user PATH does not start with install directory"
    }
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
    $env:Path = $originalProcessPath
    [Environment]::SetEnvironmentVariable("Path", $originalUserPath, "User")
    if (Test-Path $testRoot) { Remove-Item -Recurse -Force $testRoot }
}
