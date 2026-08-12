[CmdletBinding()]
param(
    [string]$Version = $(if ($env:CODEX_METER_VERSION) { $env:CODEX_METER_VERSION } else { "v0.15.0" }),
    [string]$BinDir = $(if ($env:CODEX_METER_BIN_DIR) { $env:CODEX_METER_BIN_DIR } else { Join-Path $env:LOCALAPPDATA "Programs\CodexMeter\bin" }),
    [string]$BaseUrl = $env:CODEX_METER_BASE_URL,
    [switch]$Rollback
)

$ErrorActionPreference = "Stop"
$repository = "DelicateNorman/codex-meter"
$asset = "codex-meter-windows-x86_64.exe"
$destination = Join-Path $BinDir "codex-meter.exe"
$previous = "$destination.previous"

if ($Rollback) {
    if (-not (Test-Path $previous -PathType Leaf)) {
        throw "No previous codex-meter installation is available to restore."
    }
    New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
    $swap = Join-Path $BinDir (".codex-meter.rollback." + [guid]::NewGuid().ToString("N") + ".exe")
    if (Test-Path $destination -PathType Leaf) {
        Move-Item -Force $destination $swap
    }
    try {
        Move-Item -Force $previous $destination
        & $destination --version
        if ($LASTEXITCODE -ne 0) { throw "restored executable failed its self-check" }
        if (Test-Path $swap -PathType Leaf) {
            Move-Item -Force $swap $previous
        }
    }
    catch {
        if (Test-Path $destination -PathType Leaf) {
            Move-Item -Force $destination $previous
        }
        if (Test-Path $swap -PathType Leaf) {
            Move-Item -Force $swap $destination
        }
        throw "The previous codex-meter installation failed its self-check; rollback was cancelled: $_"
    }
    Write-Host "Restored the previous installation at $destination"
    Write-Host "Existing usage data under ~/.codex-meter was not changed."
    return
}

if (-not $BaseUrl) {
    $BaseUrl = "https://github.com/$repository/releases/download/$Version"
}
$temporaryDir = Join-Path ([System.IO.Path]::GetTempPath()) ("codex-meter-" + [guid]::NewGuid().ToString("N"))

try {
    New-Item -ItemType Directory -Path $temporaryDir | Out-Null
    $download = Join-Path $temporaryDir $asset
    $checksums = Join-Path $temporaryDir "SHA256SUMS"
    Write-Host "Downloading codex-meter $Version for Windows/x86_64..."
    if (Test-Path $BaseUrl -PathType Container) {
        Copy-Item (Join-Path $BaseUrl $asset) $download
        Copy-Item (Join-Path $BaseUrl "SHA256SUMS") $checksums
    }
    else {
        Invoke-WebRequest -UseBasicParsing -Uri "$BaseUrl/$asset" -OutFile $download
        Invoke-WebRequest -UseBasicParsing -Uri "$BaseUrl/SHA256SUMS" -OutFile $checksums
    }

    $checksumLine = Get-Content $checksums | Where-Object { $_.Trim().EndsWith(" $asset") } | Select-Object -First 1
    if (-not $checksumLine) {
        throw "Release checksum does not contain $asset."
    }
    $expected = ($checksumLine -split "\s+")[0].ToLowerInvariant()
    $actual = (Get-FileHash -Algorithm SHA256 -Path $download).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        throw "Checksum verification failed for $asset."
    }

    Unblock-File -Path $download
    & $download --version | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "The downloaded codex-meter binary failed its self-check." }

    New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
    $staged = Join-Path $BinDir (".codex-meter.new." + [guid]::NewGuid().ToString("N") + ".exe")
    Copy-Item $download $staged
    Unblock-File -Path $staged
    $hadPrevious = Test-Path $destination -PathType Leaf
    if ($hadPrevious) {
        Move-Item -Force $destination $previous
    }
    try {
        Move-Item -Force $staged $destination
        & $destination --version
        if ($LASTEXITCODE -ne 0) { throw "installed executable failed its self-check" }
    }
    catch {
        if (Test-Path $destination -PathType Leaf) {
            Remove-Item -Force $destination
        }
        if ($hadPrevious -and (Test-Path $previous -PathType Leaf)) {
            Move-Item -Force $previous $destination
        }
        throw "The new codex-meter failed its self-check; the previous installation was restored: $_"
    }

    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if (-not $userPath) { $userPath = "" }
    $pathParts = @($userPath -split ";" | Where-Object { $_ })
    if ($pathParts -notcontains $BinDir) {
        $newUserPath = if ($userPath) { "$userPath;$BinDir" } else { $BinDir }
        [Environment]::SetEnvironmentVariable("Path", $newUserPath, "User")
    }
    if (($env:Path -split ";") -notcontains $BinDir) {
        $env:Path = "$BinDir;$env:Path"
    }

    Write-Host "Installed to $destination"
    if ($hadPrevious) {
        Write-Host "Previous version saved at $previous"
        Write-Host "Rollback command: .\install.ps1 -Rollback"
    }
    Write-Host "Open a new PowerShell window, then run: codex-meter"
    Write-Host "Existing usage data under ~/.codex-meter was not changed."
}
finally {
    if (Test-Path $temporaryDir) {
        Remove-Item -Recurse -Force $temporaryDir
    }
}
