[CmdletBinding()]
param(
    [string]$Version = $(if ($env:CODEX_METER_VERSION) { $env:CODEX_METER_VERSION } else { "v0.15.0" }),
    [string]$BinDir = $(if ($env:CODEX_METER_BIN_DIR) { $env:CODEX_METER_BIN_DIR } else { Join-Path $env:LOCALAPPDATA "Programs\CodexMeter\bin" }),
    [string]$BaseUrl = $env:CODEX_METER_BASE_URL
)

$ErrorActionPreference = "Stop"
$repository = "DelicateNorman/codex-meter"
$asset = "codex-meter-windows-x86_64.exe"
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

    New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
    $destination = Join-Path $BinDir "codex-meter.exe"
    Move-Item -Force -Path $download -Destination $destination
    Unblock-File -Path $destination

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

    & $destination --version
    Write-Host "Installed to $destination"
    Write-Host "Open a new PowerShell window, then run: codex-meter"
    Write-Host "Existing usage data under ~/.codex-meter was not changed."
}
finally {
    if (Test-Path $temporaryDir) {
        Remove-Item -Recurse -Force $temporaryDir
    }
}
