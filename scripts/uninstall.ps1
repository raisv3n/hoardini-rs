# Removes the installed binaries, browser registrations and Start menu shortcut.
# Your downloads and %APPDATA%\Hoardini (queue and settings) are left in place.
$ErrorActionPreference = 'Stop'
$running = Get-Process hoardini-app -ErrorAction SilentlyContinue
if ($running) { $running | Stop-Process -Force; $running | Wait-Process -Timeout 10 }
foreach ($browser in 'Google\Chrome', 'Microsoft\Edge', 'BraveSoftware\Brave-Browser', 'Chromium') {
    $key = "HKCU:\Software\$browser\NativeMessagingHosts\com.hoardini.host"
    if (Test-Path $key) { Remove-Item $key -Force }
}
$shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'Hoardini.lnk'
if (Test-Path $shortcut) { Remove-Item $shortcut -Force }
$dir = Join-Path $env:LOCALAPPDATA 'Hoardini'
if (Test-Path $dir) { Remove-Item $dir -Recurse -Force }
Write-Host 'Hoardini uninstalled.'
