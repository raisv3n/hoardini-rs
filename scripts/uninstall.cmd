@echo off
rem Double-click to uninstall Hoardini. Your downloads, queue and settings are kept.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0uninstall.ps1" %*
echo.
pause
