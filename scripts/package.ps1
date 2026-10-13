<#
.SYNOPSIS
  Builds a release and packages it as dist\Hoardini-<version>-windows-x64.zip, plus a .sha256 file.
  Used by the GitHub release workflow; also works locally.
.PARAMETER NoBuild
  Package the binaries already in target\release.
#>
param([switch]$NoBuild)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$name = "Hoardini-$version-windows-x64"

if (-not $NoBuild) {
    cargo build --release --locked
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
}

$dist = Join-Path $root 'dist'
$stage = Join-Path $dist $name
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Force (Join-Path $stage 'bin') | Out-Null

foreach ($e in 'hoardini-app.exe', 'hoardini.exe', 'hoardini-host.exe') {
    Copy-Item (Join-Path $root "target\release\$e") (Join-Path $stage 'bin')
}
Copy-Item (Join-Path $root 'extension') $stage -Recurse
foreach ($f in 'install.ps1', 'install.cmd', 'uninstall.ps1', 'uninstall.cmd') {
    Copy-Item (Join-Path $root "scripts\$f") $stage
}
Copy-Item (Join-Path $root 'LICENSE') $stage

# Repo link for the zip's README: from GitHub Actions, else the git remote, else a generic hint.
$repoUrl = if ($env:GITHUB_REPOSITORY) { "$env:GITHUB_SERVER_URL/$env:GITHUB_REPOSITORY" } else {
    $remote = try { git -C $root remote get-url origin 2>$null } catch { $null }
    if ($remote) { $remote -replace '\.git$', '' -replace '^git@github\.com:', 'https://github.com/' } else { 'see the project page on GitHub' }
}
$readme = (Get-Content (Join-Path $root 'packaging\README.txt') -Raw) -replace '\{\{REPO_URL\}\}', $repoUrl
# CRLF so it reads well in Notepad.
[IO.File]::WriteAllText((Join-Path $stage 'README.txt'), ($readme -replace "(?<!`r)`n", "`r`n"))

$zip = Join-Path $dist "$name.zip"
if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path $stage -DestinationPath $zip
$sha = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
[IO.File]::WriteAllText("$zip.sha256", "$sha  $name.zip`n")

Write-Host "Packaged $zip"
Write-Host "SHA-256  $sha"
