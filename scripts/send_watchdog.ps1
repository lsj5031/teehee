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
  - If the heartbeat is older than STALE_SECS, the watcher has not
    run for that long and this watchdog alerts. Default STALE_SECS
    is 900 (15 min): a healthy 5-min task should have touched the
    heartbeat 3 times by then.
  - A missing heartbeat file is treated as age = +infinity but only
    after the same STALE_SECS grace period (measured from the file's
    creation time sentinel, or from process start if the file has
    never existed). This prevents a false "watcher DOWN" alert on
    the first watchdog firing before TeeHeeWatch has had a chance to
    write the heartbeat. After the grace period, a still-missing
    file means TeeHeeWatch was never installed or has been removed.

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
# Sentinel file written on first watchdog run when the heartbeat is
# missing. Its creation time anchors the grace period so the watchdog
# does not false-alert before TeeHeeWatch has written the heartbeat.
$graceFile = Join-Path $root 'logs\send_watchdog.grace'

function Write-Alert([string]$msg) {
    $stamp = Get-Date -Format 'yyyy-MM-dd HH:mm:ss'
    Add-Content -Path $alertLog -Value "$stamp $msg" -Encoding utf8
    try { [System.Media.SystemSounds]::Exclamation.Play() } catch {}
    try {
        Start-Process msg.exe -ArgumentList '*', "teehee: $msg" -WindowStyle Hidden -ErrorAction Stop | Out-Null
    } catch {}
    Write-Host "[WATCHDOG] $stamp $msg"
}

# Return a hashtable with 'fresh' (bool) and 'age' (seconds, or -1 if
# the file is missing and within the grace period, or -2 if
# unreadable/corrupt).
function Get-Heartbeat {
    $r = @{ fresh = $false; age = -1 }

    # Heartbeat exists: parse and check staleness. Clear any stale
    # grace sentinel from a prior missing-heartbeat episode.
    if (Test-Path $heartbeatFile) {
        if (Test-Path $graceFile) { Remove-Item $graceFile -Force -ErrorAction SilentlyContinue }
        $raw = Get-Content $heartbeatFile -Raw -ErrorAction SilentlyContinue
        if ($null -eq $raw) {
            $r.age = -2
            return $r
        }
        $ts = 0
        if (-not [int64]::TryParse("$raw".Trim(), [ref]$ts)) {
            $r.age = -2
            return $r
        }
        $r.age = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds() - $ts
        $r.fresh = ($r.age -le $StaleSecs)
        return $r
    }

    # Heartbeat is missing. Give it the same grace period before
    # alerting, so the first watchdog firing (before TeeHeeWatch has
    # run) does not false-alert "watcher DOWN".
    $now = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
    $graceStart = $now
    if (Test-Path $graceFile) {
        try {
            $graceStart = [int64](Get-Content $graceFile -Raw -ErrorAction SilentlyContinue).Trim()
        } catch {}
    } else {
        # First time we've noticed the heartbeat is missing: stamp the
        # sentinel so the grace window starts now, not at some future
        # check.
        try {
            Set-Content -Path $graceFile -Value $now -Encoding ascii
        } catch {}
    }
    $graceAge = $now - $graceStart
    if ($graceAge -ge $StaleSecs) {
        # Grace expired — heartbeat is genuinely missing (TeeHeeWatch
        # was never installed or has been removed).
        $r.age = -1
    } else {
        # Still within grace: treat as fresh so we don't alert yet.
        $r.fresh = $true
        $r.age = -1
    }
    return $r
}

# --- main ------------------------------------------------------------
$hb = Get-Heartbeat
if (-not $hb.fresh) {
    $ageStr = if ($hb.age -lt 0) { 'unknown' } else { $hb.age }
    Write-Alert ("watcher DOWN: heartbeat missing or stale (age=$ageStr s, stale threshold $StaleSecs s) - TeeHeeWatch task is not running")
}
