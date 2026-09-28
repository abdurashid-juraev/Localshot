@echo off
setlocal
cd /d "%~dp0"

echo [LocalShot] Windows Installer ishga tushmoqda...
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1"

if %ERRORLEVEL% NEQ 0 (
    echo.
    echo [Xato] O'rnatishda xatolik yuz berdi.
) else (
    echo.
    echo [Muvaffaqiyatli] O'rnatish yakunlandi!
)

echo.
pause
