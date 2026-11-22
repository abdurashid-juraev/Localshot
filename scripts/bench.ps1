param(
    [string]$Binary = ".\target\release\localshot.exe"
)

if (-not (Test-Path $Binary)) {
    Write-Error "Release binary not found at $Binary. Build with: cargo build --release"
    exit 1
}

Write-Host "=== LocalShot Windows Performance Benchmark ==="
Write-Host "Binary: $Binary"

$env:LOCALSHOT_PERF = "1"
$proc = Start-Process -FilePath $Binary -ArgumentList "--daemon" -PassThru
Start-Sleep -Seconds 2

$proc.Refresh()
if (-not $proc.HasExited) {
    $rssMb = [math]::Round($proc.WorkingSet64 / 1MB, 2)
    Write-Host "Idle Daemon WorkingSet: $rssMb MB ($($proc.WorkingSet64) bytes)"
    & $Binary --quit | Out-Null
    $proc.WaitForExit(3000) | Out-Null
} else {
    Write-Warning "Daemon exited early with code $($proc.ExitCode)"
}

Write-Host "Running capture-to-visible timing (press Esc when overlay appears)..."
& $Binary --oneshot 2>&1 | Select-String "\[PERF\]"
