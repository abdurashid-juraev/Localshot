<#
.SYNOPSIS
    LocalShot Windows Installer & Launcher Creator
.DESCRIPTION
    Installs LocalShot to %LOCALAPPDATA%\LocalShot, embeds icon, and creates
    Desktop and Start Menu launchers.
#>

$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$rootDir = Split-Path -Parent $scriptDir
Set-Location $rootDir

Write-Host "========================================" -ForegroundColor Cyan
Write-Host "       LocalShot Windows Installer      " -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan

# 1. Locate or Build localshot.exe
$releaseExe = Join-Path $rootDir "target\release\localshot.exe"
$debugExe   = Join-Path $rootDir "target\debug\localshot.exe"
$rootExe    = Join-Path $rootDir "localshot.exe"

$sourceExe = $null

if (Test-Path $releaseExe) {
    $sourceExe = $releaseExe
} elseif (Test-Path $rootExe) {
    $sourceExe = $rootExe
} elseif (Test-Path $debugExe) {
    $sourceExe = $debugExe
} else {
    Write-Host "[*] Executable not found. Checking Rust compiler..." -ForegroundColor Yellow
    
    $cargoCmd = Get-Command "cargo" -ErrorAction SilentlyContinue
    if (-not $cargoCmd) {
        $defaultCargo = "$HOME\.cargo\bin\cargo.exe"
        if (Test-Path $defaultCargo) {
            $env:Path = "$HOME\.cargo\bin;$env:Path"
            $cargoCmd = Get-Command "cargo" -ErrorAction SilentlyContinue
        }
    }

    if ($cargoCmd) {
        Write-Host "[*] Building release binary with cargo (this may take a minute)..." -ForegroundColor Green
        & cargo build --release
        if (Test-Path $releaseExe) {
            $sourceExe = $releaseExe
        }
    }
}

if (-not $sourceExe -or -not (Test-Path $sourceExe)) {
    Write-Host "[!] localshot.exe topilmadi!" -ForegroundColor Red
    Write-Host "    Iltimos, avval dasturni yig'ing (build qiling):" -ForegroundColor Yellow
    Write-Host "    1. Rust o'rnatilgan bo'lsa: cargo build --release" -ForegroundColor White
    Write-Host "    2. Yoki tayyor 'localshot.exe' faylini ushbu papkaga qo'ying." -ForegroundColor White
    exit 1
}

Write-Host "[+] Topilgan binary: $sourceExe" -ForegroundColor Green

# 2. Icon tekshirish
$iconSource = Join-Path $rootDir "assets\icon.ico"
if (-not (Test-Path $iconSource)) {
    $makeIcon = Join-Path $scriptDir "make_icon.ps1"
    if (Test-Path $makeIcon) {
        Write-Host "[*] assets\icon.ico yaratilmoqda..." -ForegroundColor Yellow
        & powershell -ExecutionPolicy Bypass -File $makeIcon
    }
}

# 3. O'rnatish manzili: %LOCALAPPDATA%\LocalShot
$installDir = Join-Path $env:LOCALAPPDATA "LocalShot"
if (-not (Test-Path $installDir)) {
    New-Item -ItemType Directory -Path $installDir -Force | Out-Null
}

$destExe  = Join-Path $installDir "localshot.exe"
$destIcon = Join-Path $installDir "icon.ico"

Write-Host "[*] Fayllar $installDir papkasiga ko'chirilmoqda..." -ForegroundColor Cyan
Copy-Item -Path $sourceExe -Destination $destExe -Force
if (Test-Path $iconSource) {
    Copy-Item -Path $iconSource -Destination $destIcon -Force
}

# 4. Shortcut (Launcher) yaratish funksiyasi
$wshShell = New-Object -ComObject WScript.Shell

function Create-LauncherShortcut {
    param(
        [string]$ShortcutPath,
        [string]$TargetPath,
        [string]$IconPath,
        [string]$Description
    )

    $shortcut = $wshShell.CreateShortcut($ShortcutPath)
    $shortcut.TargetPath = $TargetPath
    $shortcut.WorkingDirectory = [System.IO.Path]::GetDirectoryName($TargetPath)
    $shortcut.Description = $Description
    if ($IconPath -and (Test-Path $IconPath)) {
        $shortcut.IconLocation = "$IconPath,0"
    } else {
        $shortcut.IconLocation = "$TargetPath,0"
    }
    # Hotkey: Ctrl+Alt+S (Ixtiyoriy, qulay skrinshot olish uchun)
    $shortcut.Hotkey = "CTRL+ALT+S"
    $shortcut.Save()
}

# 4.1. Desktop Shortcut (Ish stoli)
$desktopDir = [System.Environment]::GetFolderPath([System.Environment+SpecialFolder]::Desktop)
$desktopLnk = Join-Path $desktopDir "LocalShot.lnk"
Create-LauncherShortcut -ShortcutPath $desktopLnk -TargetPath $destExe -IconPath $destIcon -Description "LocalShot - Offline Screenshot Tool (Ctrl+Alt+S)"

# 4.2. Start Menu Shortcut (Pusk menyusi / Windows Search)
$startProgramsDir = [System.Environment]::GetFolderPath([System.Environment+SpecialFolder]::Programs)
$startMenuLnk = Join-Path $startProgramsDir "LocalShot.lnk"
Create-LauncherShortcut -ShortcutPath $startMenuLnk -TargetPath $destExe -IconPath $destIcon -Description "LocalShot - Offline Screenshot Tool"

# 5. User PATH ga qo'shish (Win + R orqali yoki terminaldan 'localshot' deb ochish uchun)
$userPath = [System.Environment]::GetEnvironmentVariable("Path", "User")
if ($userPath -notlike "*$installDir*") {
    [System.Environment]::SetEnvironmentVariable("Path", "$userPath;$installDir", "User")
    Write-Host "[+] LocalShot foydalanuvchi PATH muhitiga qo'shildi." -ForegroundColor Green
}

Write-Host "`n========================================" -ForegroundColor Green
Write-Host "   LocalShot muvaffaqiyatli o'rnatildi! " -ForegroundColor Green
Write-Host "========================================" -ForegroundColor Green
Write-Host "[*] Desktop yorlig'i:  $desktopLnk" -ForegroundColor White
Write-Host "[*] Start Menyu:       $startMenuLnk" -ForegroundColor White
Write-Host "[*] O'rnatilgan joy:   $destExe" -ForegroundColor White
Write-Host "[*] Tezkor klaviatura: Ctrl + Alt + S" -ForegroundColor Yellow
Write-Host "    (Istalgan vaqtda Ctrl+Alt+S bosib skrinshot olishingiz mumkin!)" -ForegroundColor Gray
