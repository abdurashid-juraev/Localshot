@echo off
setlocal
cd /d "%~dp0"

echo [LocalShot] Professional Installer yaratilmoqda...
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0build_installer.ps1"

echo.
pause
