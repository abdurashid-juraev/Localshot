<#
.SYNOPSIS
    Professional Installer Builder for LocalShot
.DESCRIPTION
    Compiles the release executable and packages it into a professional
    Windows setup file (dist\LocalShot-Setup-0.1.0.exe) using Inno Setup.
#>

$ErrorActionPreference = "Stop"
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
Set-Location $scriptDir

Write-Host "================================================" -ForegroundColor Cyan
Write-Host "   LocalShot Professional Installer Yaratuvchi   " -ForegroundColor Cyan
Write-Host "================================================" -ForegroundColor Cyan

# 1. Binary tekshirish yoki yig'ish (Cargo build)
$releaseExe = Join-Path $scriptDir "target\release\localshot.exe"

if (-not (Test-Path $releaseExe)) {
    Write-Host "`n[*] localshot.exe topilmadi. Cargo orqali yig'ish (build) tekshirilmoqda..." -ForegroundColor Yellow
    
    $cargo = Get-Command "cargo" -ErrorAction SilentlyContinue
    if (-not $cargo -and (Test-Path "$HOME\.cargo\bin\cargo.exe")) {
        $env:Path = "$HOME\.cargo\bin;$env:Path"
        $cargo = Get-Command "cargo" -ErrorAction SilentlyContinue
    }

    if ($cargo) {
        Write-Host "[*] Release versiya yig'ilmoqda (cargo build --release)..." -ForegroundColor Green
        & cargo build --release
        if ($LASTEXITCODE -ne 0 -or -not (Test-Path $releaseExe)) {
            Write-Host "`n[!] Cargo build xatolik bilan tugadi!" -ForegroundColor Red
            Write-Host "    Iltimos, yuqoridagi xatolikni bartaraf qiling." -ForegroundColor Yellow
            exit 1
        }
    } else {
        Write-Host "`n[!] Rust (cargo) topilmadi!" -ForegroundColor Red
        Write-Host "    Iltimos, avval dasturni build qiling yoki 'target\release\localshot.exe' faylini joylashtiring." -ForegroundColor Yellow
        Write-Host "    Agar Rust o'rnatilmagan bo'lsa: https://rustup.rs orqali o'rnating." -ForegroundColor White
        exit 1
    }
}

if (-not (Test-Path $releaseExe)) {
    Write-Host "`n[!] target\release\localshot.exe topilmadi!" -ForegroundColor Red
    exit 1
}

Write-Host "[+] Binary tayyor: $releaseExe" -ForegroundColor Green

# 2. Icon mavjudligini tekshirish
$iconPath = Join-Path $scriptDir "assets\icon.ico"
if (-not (Test-Path $iconPath)) {
    Write-Host "[*] assets\icon.ico yaratilmoqda..." -ForegroundColor Yellow
    & powershell -ExecutionPolicy Bypass -File (Join-Path $scriptDir "assets\make_icon.ps1")
}

# 3. Inno Setup (ISCC.exe) tekshirish
Write-Host "`n[*] Inno Setup tekshirilmoqda..." -ForegroundColor Cyan

$isccPath = $null
$possiblePaths = @(
    (Join-Path $env:LOCALAPPDATA "Programs\Inno Setup 6\ISCC.exe"),
    "C:\Program Files (x86)\Inno Setup 6\ISCC.exe",
    "C:\Program Files\Inno Setup 6\ISCC.exe"
)

foreach ($p in $possiblePaths) {
    if (Test-Path $p) {
        $isccPath = $p
        break
    }
}

if (-not $isccPath) {
    $cmd = Get-Command "iscc" -ErrorAction SilentlyContinue
    if ($cmd) { $isccPath = $cmd.Source }
}

# Agar Inno Setup o'rnatilmagan bo'lsa, winget orqali o'rnatish
if (-not $isccPath) {
    Write-Host "[!] Inno Setup kompilyatori topilmadi." -ForegroundColor Yellow
    Write-Host "[*] Winget orqali Inno Setup o'rnatilmoqda..." -ForegroundColor Cyan
    
    $winget = Get-Command "winget" -ErrorAction SilentlyContinue
    if ($winget) {
        & winget install --id JRSoftware.InnoSetup -e --source winget --accept-package-agreements --accept-source-agreements --silent
        
        # Qayta tekshirish
        foreach ($p in $possiblePaths) {
            if (Test-Path $p) {
                $isccPath = $p
                break
            }
        }
    }
}

if (-not $isccPath) {
    Write-Host "`n[!] Inno Setup topilmadi yoki o'rnatib bo'lmadi." -ForegroundColor Red
    Write-Host "    Iltimos, bepul Inno Setup dasturini yuklab oling va o'rnating:" -ForegroundColor Yellow
    Write-Host "    👉 https://jrsoftware.org/isdl.php" -ForegroundColor Cyan
    Write-Host "    O'rnatgandan so'ng, ushbu skriptni qayta ishga tushiring." -ForegroundColor White
    exit 1
}

Write-Host "[+] Inno Setup topildi: $isccPath" -ForegroundColor Green

# 4. Installer yaratish
Write-Host "`n[*] Professional Setup.exe yaratilmoqda..." -ForegroundColor Cyan
$issFile = Join-Path $scriptDir "installer.iss"
& "$isccPath" "$issFile"

$distDir = Join-Path $scriptDir "dist"
$setupItem = Get-ChildItem -Path $distDir -Filter "LocalShot-Setup-*.exe" -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First 1
$setupExe = if ($setupItem) { $setupItem.FullName } else { $null }

if ($setupExe -and (Test-Path $setupExe)) {
    Write-Host "`n================================================" -ForegroundColor Green
    Write-Host "   PROFESSIONAL INSTALLER TAYYOR BO'LDI!       " -ForegroundColor Green
    Write-Host "================================================" -ForegroundColor Green
    Write-Host "[*] O'rnatgich fayl: $setupExe" -ForegroundColor White
    Write-Host "[*] Hajmi: $((Get-Item $setupExe).Length / 1MB | ForEach-Object { '{0:N2} MB' -f $_ })" -ForegroundColor White
    
    # Ochish
    explorer.exe "/select,$setupExe"
} else {
    Write-Host "[!] Setup yaratishda muammo yuz berdi." -ForegroundColor Red
}
