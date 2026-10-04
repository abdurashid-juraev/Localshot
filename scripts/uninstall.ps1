$installDir = Join-Path $env:LOCALAPPDATA "LocalShot"
$desktopLnk = Join-Path ([System.Environment]::GetFolderPath([System.Environment+SpecialFolder]::Desktop)) "LocalShot.lnk"
$startMenuLnk = Join-Path ([System.Environment]::GetFolderPath([System.Environment+SpecialFolder]::Programs)) "LocalShot.lnk"

Write-Host "LocalShot tizimdan o'chirilmoqda..." -ForegroundColor Yellow

if (Test-Path $desktopLnk) { Remove-Item $desktopLnk -Force }
if (Test-Path $startMenuLnk) { Remove-Item $startMenuLnk -Force }
if (Test-Path $installDir) { Remove-Item $installDir -Recurse -Force }

# Remove from PATH
$userPath = [System.Environment]::GetEnvironmentVariable("Path", "User")
if ($userPath -like "*$installDir*") {
    $newPath = ($userPath.Split(';') | Where-Object { $_ -ne $installDir }) -join ';'
    [System.Environment]::SetEnvironmentVariable("Path", $newPath, "User")
}

Write-Host "LocalShot muvaffaqiyatli o'chirildi (Uninstalled)." -ForegroundColor Green
