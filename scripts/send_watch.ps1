<#
send_watch.ps1 - monitor the teehee sender's JSONL log and alert on trouble.

This is the operator's single monitoring/alerting entry point for the
sender. It replaces the old bash send_watch.sh, which could not be
launched reliably from Task Scheduler (Git Bash dies in that context);
PowerShell is native and task-launchable.

Two modes:

  -Watch  (default)  Foreground watcher for interactive use. Polls
                     logs\send.jsonl every 10 s and alerts on state
                     transitions (ring fill, drift saturation, silent
                     sender). Run via `just watch-send`.

  -Check              One-shot health check for the TeeHeeWatch
                     scheduled task (runs every 5 min). Examines the
                     recent send_stats records, compares against a
                     state file (logs\send_watch.state), and alerts
                     only on NEW problems and RECOVERIES - so a stuck
                     sender alerts once, not every 5 minutes.

Checks (both modes):

  1. capture_ring_ms exceeds 50% of the capture-buffer cap
     -> delay is building, or the encode loop is being starved
        (CPU contention / stall). With the drift compensator this
        should be rare; when it fires, check machine load.
  2. send_drift_ppm saturates at the +/-150 clamp
     -> stall or long-pause recovery in progress; it settles on
        its own within seconds to minutes. Worth knowing about.
  3. no send_stats line newer than STALE_SECS
     -> the sender exited or hung. In -Check mode STALE_SECS is
        240 s: the sender task self-heals within ~2 min, so stats
        older than 4 min mean self-healing has FAILED and the
        operator should look. In -Watch mode it is 20 s.

The ring cap is auto-detected from the most recent `send_start`
event's capture_buffer_ms (falls back to 400, the deployed setting).

Alerting: appends a timestamped line to logs\send_alerts.log and plays
an audible alert. In -Check mode it also best-effort pops a message
box via msg.exe so a headless scheduled run still reaches the desktop.

Usage:
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\send_watch.ps1
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\send_watch.ps1 -Check
#>
param(
    [switch]$Check,
    [string]$Log = ""
)

$ErrorActionPreference = 'Continue'

# --- paths -----------------------------------------------------------
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$root = Split-Path -Parent $here
if (-not $Log) { $Log = Join-Path $root 'logs\send.jsonl' }
$alertLog = Join-Path $root 'logs\send_alerts.log'
$stateFile = Join-Path $root 'logs\send_watch.state'

$RING_WARN_PCT = 50
$PPM_SAT = 149.0
if ($Check) {
    $STALE_SECS = 240   # > 2-min self-heal window -> self-heal has failed
    $POLL_SECS = 0
} else {
    $STALE_SECS = 20
    $POLL_SECS = 10
}
$TAIL_LINES = 5000

# Heartbeat: every run (Check or Watch) touches this file with a UTC
# unix timestamp. The separate watchdog (send_watchdog.ps1, TeeHeeWatchdog
# scheduled task) alerts if this file goes stale - that means THIS task
# (TeeHeeWatch) stopped running and nothing is watching the sender.
$heartbeatFile = Join-Path $root 'logs\send_watch.heartbeat'

function Write-Heartbeat {
    try {
        Set-Content -Path $heartbeatFile -Value ([DateTimeOffset]::UtcNow.ToUnixTimeSeconds()) -Encoding ascii
    } catch {}
}

# --- helpers ---------------------------------------------------------
function Write-Alert([string]$msg) {
    $stamp = Get-Date -Format 'yyyy-MM-dd HH:mm:ss'
    Add-Content -Path $alertLog -Value "$stamp $msg" -Encoding utf8
    try { [System.Media.SystemSounds]::Exclamation.Play() } catch {}
    if ($Check) {
        # Best-effort desktop popup; never block the check on it.
        try {
            Start-Process msg.exe -ArgumentList '*', "teehee: $msg" -WindowStyle Hidden -ErrorAction Stop | Out-Null
        } catch {}
    }
    Write-Host "[ALERT] $stamp $msg"
}

function Write-Recovery([string]$msg) {
    $stamp = Get-Date -Format 'yyyy-MM-dd HH:mm:ss'
    Add-Content -Path $alertLog -Value "$stamp $msg" -Encoding utf8
    Write-Host "[OK] $stamp $msg"
}

# Read the tail of the JSONL log and extract the latest send_start cap
# and the latest send_stats record. Returns a hashtable.
function Read-LogSnapshot {
    $snap = @{
        cap_ms = 400
        stats  = $null
        now    = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
    }
    if (-not (Test-Path $Log)) { return $snap }
    $lines = Get-Content $Log -Tail $TAIL_LINES -ErrorAction SilentlyContinue
    foreach ($line in $lines) {
        if ($line -notmatch '"event":"send_') { continue }
        try { $o = $line | ConvertFrom-Json } catch { continue }
        if ($o.event -eq 'send_start') {
            if ($o.capture_buffer_ms -gt 0) { $snap.cap_ms = [int]$o.capture_buffer_ms }
        } elseif ($o.event -eq 'send_stats') {
            $snap.stats = $o
        }
    }
    return $snap
}

# Evaluate the active trouble conditions for the snapshot. Returns a
# list of @{ name; detail } for each active condition (down/ring/drift).
function Get-Conditions([hashtable]$snap) {
    $list = @()
    $stats = $snap.stats
    if ($null -eq $stats) {
        $list += @{ name = 'down'; detail = "sender DOWN: no send_stats found in $Log (never started, or log missing)" }
        return $list
    }
    $age = $snap.now - [int64]($stats.ts_ms / 1000)
    if ($age -gt $STALE_SECS) {
        $list += @{ name = 'down'; detail = "sender DOWN: no send_stats for $age s (self-heal has not recovered it)" }
    }
    $ring = [double]$stats.capture_ring_ms
    $thresh = $snap.cap_ms * $RING_WARN_PCT / 100
    if ($ring -gt $thresh) {
        $list += @{ name = 'ring'; detail = "capture_ring_ms=${ring} exceeds ${thresh}ms ($($snap.cap_ms)ms cap) - encode loop starved or delay building" }
    }
    $ppm = [double]$stats.send_drift_ppm
    if ([Math]::Abs($ppm) -ge $PPM_SAT) {
        $list += @{ name = 'drift'; detail = "send_drift_ppm=${ppm} saturated at +/-150 - stall/pause recovery (will settle)" }
    }
    return $list
}

function Get-RecoveryMessage([string]$name) {
    switch ($name) {
        'down'  { return "sender back up (send_stats resumed)" }
        'ring'  { return "capture ring back to normal" }
        'drift' { return "drift compensator unsaturating" }
    }
    return $null
}

# One-shot health check (scheduled task mode): alert on new problems,
# log recoveries, persist state so a problem alerts only once.
function Invoke-Check {
    Write-Heartbeat
    $snap = Read-LogSnapshot
    $active = @(Get-Conditions $snap)

    $prev = @()
    if (Test-Path $stateFile) {
        $prev = @((Get-Content $stateFile -Raw).Trim() -split ',' | Where-Object { $_ })
    }
    $prevSet = @{}; foreach ($p in $prev) { $prevSet[$p] = $true }
    $activeSet = @{}; foreach ($a in $active) { $activeSet[$a.name] = $true }

    foreach ($c in $active) {
        if (-not $prevSet.ContainsKey($c.name)) { Write-Alert $c.detail }
    }
    foreach ($p in $prev) {
        if (-not $activeSet.ContainsKey($p)) {
            $msg = Get-RecoveryMessage $p
            if ($msg) { Write-Recovery $msg }
        }
    }

    $state = ($active | ForEach-Object { $_.name }) -join ','
    Set-Content -Path $stateFile -Value $state -Encoding ascii
}

# Interactive foreground watcher: poll every $POLL_SECS, alert on
# transitions, track alarms in memory.
function Invoke-Watch {
    if (-not (Test-Path $Log)) {
        Write-Host "send_watch: log not found: $Log" -ForegroundColor Red
        exit 1
    }
    Write-Heartbeat
    $prev = @{}
    while ($true) {
        $snap = Read-LogSnapshot
        $active = @(Get-Conditions $snap)
        $activeSet = @{}; foreach ($a in $active) { $activeSet[$a.name] = $a }

        foreach ($c in $active) {
            if (-not $prev.ContainsKey($c.name)) { Write-Alert $c.detail }
        }
        foreach ($k in $prev.Keys) {
            if (-not $activeSet.ContainsKey($k)) {
                $msg = Get-RecoveryMessage $k
                if ($msg) { Write-Recovery $msg }
            }
        }
        $prev = $activeSet
        Write-Heartbeat
        Start-Sleep -Seconds $POLL_SECS
    }
}

# --- dispatch --------------------------------------------------------
if ($Check) {
    Invoke-Check
} else {
    Invoke-Watch
}
