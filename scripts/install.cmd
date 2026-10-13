@echo off
rem Double-click to install Hoardini. Runs install.ps1 without changing the system's script policy.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" %*
echo.
pause
