<#
.SYNOPSIS
  Installs Hoardini for the current user (no admin rights needed) and registers the browser
  native-messaging host for Chrome, Edge, Brave and Chromium.

  Works from two places:
    - a release zip (exes in .\bin next to this script): installs those;
    - the source repo (this script in .\scripts): builds with cargo first.

  Then load %LOCALAPPDATA%\Hoardini\extension as an unpacked extension in chrome://extensions or
  edge://extensions (Developer mode).
.PARAMETER NoBuild
  Source repo only: install the binaries already in target\release.
.PARAMETER Yes
  Don't ask questions: install ffmpeg with winget if it is missing.
.PARAMETER NoFfmpeg
  Don't offer to install ffmpeg.
#>
param([switch]$NoBuild, [switch]$Yes, [switch]$NoFfmpeg)
$ErrorActionPreference = 'Stop'

$release = Test-Path (Join-Path $PSScriptRoot 'bin\hoardini-app.exe')
if ($release) {
    $root = $PSScriptRoot
    $from = Join-Path $root 'bin'
    # Files from a downloaded zip carry the Mark of the Web; installed copies shouldn't trigger warnings each launch.
    Get-ChildItem $root -Recurse -File | Unblock-File
} else {
    $root = Split-Path -Parent $PSScriptRoot
    $from = Join-Path $root 'target\release'
    if (-not $NoBuild) {
        Push-Location $root
        cargo build --release
        $ok = $LASTEXITCODE -eq 0
        Pop-Location
        if (-not $ok) { throw 'cargo build failed' }
    }
}

$home_ = Join-Path $env:LOCALAPPDATA 'Hoardini'
$bin = Join-Path $home_ 'bin'
$exes = 'hoardini.exe', 'hoardini-host.exe', 'hoardini-app.exe'

# A running app would lock its exe. (Downloads in progress resume on next launch.)
$running = Get-Process hoardini-app -ErrorAction SilentlyContinue
if ($running) { $running | Stop-Process -Force; $running | Wait-Process -Timeout 10 }
New-Item -ItemType Directory -Force $bin | Out-Null
# Retry: an exited process or an antivirus scan can hold the old exe for a moment.
foreach ($e in $exes) {
    for ($try = 1; ; $try++) {
        try { Copy-Item (Join-Path $from $e) $bin -Force -ErrorAction Stop; break }
        catch { if ($try -ge 10) { throw }; Start-Sleep -Milliseconds 500 }
    }
}

# The manifest's public key pins the extension ID: first 32 hex chars of SHA-256(key), mapped 0-f -> a-p.
$manifest = Get-Content (Join-Path $root 'extension\manifest.json') -Raw | ConvertFrom-Json
$der = [Convert]::FromBase64String($manifest.key)
$hash = [System.Security.Cryptography.SHA256]::Create().ComputeHash($der)
$hex = -join ($hash | ForEach-Object { $_.ToString('x2') })
$extId = -join ($hex.Substring(0, 32).ToCharArray() | ForEach-Object { [char]([int][char]'a' + [Convert]::ToInt32([string]$_, 16)) })

# The browser loads the unpacked extension from this copy, so it survives moving or deleting the download.
$extDir = Join-Path $home_ 'extension'
if (Test-Path $extDir) { Remove-Item $extDir -Recurse -Force }
Copy-Item (Join-Path $root 'extension') $extDir -Recurse

$hostManifest = Join-Path $bin 'com.hoardini.host.json'
$json = [ordered]@{
    name            = 'com.hoardini.host'
    description     = 'Hoardini native host'
    path            = (Join-Path $bin 'hoardini-host.exe')
    type            = 'stdio'
    allowed_origins = @("chrome-extension://$extId/")
} | ConvertTo-Json
[IO.File]::WriteAllText($hostManifest, $json, (New-Object System.Text.UTF8Encoding $false))

foreach ($browser in 'Google\Chrome', 'Microsoft\Edge', 'BraveSoftware\Brave-Browser', 'Chromium') {
    $key = "HKCU:\Software\$browser\NativeMessagingHosts\com.hoardini.host"
    New-Item -Path $key -Force | Out-Null
    Set-Item -Path $key -Value $hostManifest
}

# Start menu shortcut.
$shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'Hoardini.lnk'
$shell = New-Object -ComObject WScript.Shell
$lnk = $shell.CreateShortcut($shortcut)
$lnk.TargetPath = Join-Path $bin 'hoardini-app.exe'
$lnk.WorkingDirectory = $bin
$lnk.Description = 'Hoardini download manager'
$lnk.Save()

Write-Host "Installed to $bin" -ForegroundColor Green

# ffmpeg is optional: only MP4 conversion and HLS streams with separate audio need it.
# Looked up the same way the app does: PATH, then winget's links folder.
$wingetLink = Join-Path $env:LOCALAPPDATA 'Microsoft\WinGet\Links\ffmpeg.exe'
$hasFfmpeg = (Get-Command ffmpeg.exe -ErrorAction SilentlyContinue) -or (Test-Path $wingetLink)
if (-not $hasFfmpeg -and -not $NoFfmpeg) {
    $winget = Get-Command winget.exe -ErrorAction SilentlyContinue
    if (-not $winget) {
        Write-Host 'ffmpeg is not installed. MP4 conversion needs it: https://ffmpeg.org/download.html' -ForegroundColor Yellow
    } else {
        $answer = if ($Yes) { 'y' } else { Read-Host 'ffmpeg is not installed. MP4 conversion needs it. Install it now with winget? [Y/n]' }
        if ($answer -eq '' -or $answer -match '^[yY]') {
            winget install --id Gyan.FFmpeg -e --accept-package-agreements --accept-source-agreements
        } else {
            Write-Host 'Skipped. You can install it later with: winget install Gyan.FFmpeg'
        }
    }
}

Write-Host ''
Write-Host 'Last step: add the browser extension.' -ForegroundColor Cyan
Write-Host '  1. Open chrome://extensions (or edge://extensions) and turn on Developer mode.'
Write-Host "  2. Click 'Load unpacked' and choose: $extDir"
Write-Host 'After updating Hoardini, click the reload icon on the extension card.'
