<#
send_watchdog.ps1 - watchdog for the watcher itself.

The TeeHeeWatch scheduled task runs send_watch.ps1 -Check every 5 min.
That chain has a single point of failure: if the scheduled task is
disabled, its trigger breaks, PowerShell is broken, or the task is
removed, send_watch silently stops alerting and a dead sender would go
unnoticed indefinitely.

This script closes that loop. It runs on its own schedule (TeeHeeWatchdog
scheduled task, every 10 min) and checks the watcher heartbeat:

  - logs\send_watch.heartbeat is written by every send_watch run
    (-Check or -Watch) with a UTC unix timestamp.
  - If the heartbeat is missing entirely, or older than
    STALE_SECS, the watcher has not run for that long and this
    watchdog alerts. Default STALE_SECS is 900 (15 min): a healthy
    5-min task should have touched the heartbeat 3 times by then.

Alerting: appends a timestamped line to logs\send_alerts.log, plays an
audible alert, and best-effort pops a message box via msg.exe - same
channels as the sender watcher.

Usage:
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\send_watchdog.ps1
#>
param(
    [int]$StaleSecs = 900
)

$ErrorActionPreference = 'Continue'

# --- paths -----------------------------------------------------------
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$root = Split-Path -Parent $here
$alertLog = Join-Path $root 'logs\send_alerts.log'
$heartbeatFile = Join-Path $root 'logs\send_watch.heartbeat'

function Write-Alert([string]$msg) {
    $stamp = Get-Date -Format 'yyyy-MM-dd HH:mm:ss'
    Add-Content -Path $alertLog -Value "$stamp $msg" -Encoding utf8
    try { [System.Media.SystemSounds]::Exclamation.Play() } catch {}
    try {
        Start-Process msg.exe -ArgumentList '*', "teehee: $msg" -WindowStyle Hidden -ErrorAction Stop | Out-Null
    } catch {}
    Write-Host "[WATCHDOG] $stamp $msg"
}

function Test-Heartbeat {
    if (-not (Test-Path $heartbeatFile)) {
        return $false
    }
    $raw = Get-Content $heartbeatFile -Raw -ErrorAction SilentlyContinue
    $ts = 0
    if (-not [int64]::TryParse("$raw".Trim(), [ref]$ts)) {
        return $false
    }
    $age = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds() - $ts
    return ($age -le $StaleSecs)
}

# --- main ------------------------------------------------------------
if (-not (Test-Heartbeat)) {
    $age = if (Test-Path $heartbeatFile) {
        [DateTimeOffset]::UtcNow.ToUnixTimeSeconds() - [int64]((Get-Content $heartbeatFile -Raw -ErrorAction SilentlyContinue).Trim())
    } else { -1 }
    Write-Alert ("watcher DOWN: heartbeat missing or stale (age=$age s, stale threshold $StaleSecs s) - TeeHeeWatch task is not running")
}
