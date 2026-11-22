#!/usr/bin/env bash
set -euo pipefail

BIN="${1:-./target/release/localshot}"
if [ ! -x "$BIN" ]; then
  echo "Release binary not found at $BIN. Build with: cargo build --release" >&2
  exit 1
fi

echo "=== LocalShot Linux Performance Benchmark ==="
echo "Binary: $BIN"
echo "Host: $(uname -srm)"

# 1. Measure idle daemon RSS
LOCALSHOT_PERF=1 "$BIN" --daemon >/tmp/localshot_bench_daemon.log 2>&1 &
DAEMON_PID=$!
sleep 2

if ps -p "$DAEMON_PID" >/dev/null 2>&1; then
  RSS_KB=$(awk '/VmRSS:/ {print $2}' "/proc/${DAEMON_PID}/status" 2>/dev/null || echo "0")
  RSS_MB=$(awk "BEGIN {printf \"%.2f\", ${RSS_KB}/1024}")
  echo "Idle Daemon VmRSS: ${RSS_KB} kB (${RSS_MB} MB)"
  "$BIN" --quit >/dev/null 2>&1 || true
  wait "$DAEMON_PID" 2>/dev/null || true
else
  echo "Daemon exited early (no display/tray in headless environment?):"
  cat /tmp/localshot_bench_daemon.log || true
fi

# 2. Measure capture-to-visible latency (requires active DISPLAY / WAYLAND_DISPLAY)
if [ -n "${DISPLAY:-}" ] || [ -n "${WAYLAND_DISPLAY:-}" ]; then
  echo "Running capture-to-visible timing (press Esc when overlay appears)..."
  LOCALSHOT_PERF=1 "$BIN" --oneshot 2>&1 | grep '\[PERF\]' || true
else
  echo "Skipping capture-to-visible latency: no DISPLAY or WAYLAND_DISPLAY set."
fi

rm -f /tmp/localshot_bench_daemon.log
