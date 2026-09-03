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
        its own within seconds to minutes. Only alerted while
        non-silent audio has been captured recently
        ($DriftQuietSuppressSecs) - a saturated tracker while the
        capture engine is silent/idle is expected (ring drained,
        nothing streaming) and is not an operator event. Repeated
        transitions are rate-limited so they do not bury more
        important alerts.
  3. no send_stats line newer than STALE_SECS
     -> the sender exited or hung. In -Check mode STALE_SECS is
        240 s: the sender task self-heals within ~2 min, so stats
        older than 4 min mean self-healing has FAILED and the
        operator should look. In -Watch mode it is 20 s.
  4. capture_nonzero_samples does not change for CaptureQuietHours
     -> WASAPI loopback has produced no non-silent audio. This can be
        normal when the PC is idle, so it alerts once and recovers
        only when non-silent samples resume.

The ring cap is auto-detected from the most recent `send_start`
event's capture_buffer_ms (falls back to 1200, the deployed setting).

Alerting: appends a timestamped line to logs\send_alerts.log and plays
an audible alert. In -Check mode it also best-effort pops a message
box via msg.exe so a headless scheduled run still reaches the desktop.

Usage:
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\send_watch.ps1
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\send_watch.ps1 -Check
#>
param(
    [switch]$Check,
    [string]$Log = "",
    [int]$CaptureQuietHours = 4,
    [int]$DriftAlertCooldownMinutes = 360,
    [int]$DriftQuietSuppressSecs = 120
)

$ErrorActionPreference = 'Continue'

# --- paths -----------------------------------------------------------
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$root = Split-Path -Parent $here
if (-not $Log) { $Log = Join-Path $root 'logs\send.jsonl' }
$alertLog = Join-Path $root 'logs\send_alerts.log'
$stateFile = Join-Path $root 'logs\send_watch.state'
if ($Check) {
    $metaFile = Join-Path $root 'logs\send_watch.meta.json'
} else {
    $metaFile = Join-Path $root 'logs\send_watch.foreground.meta.json'
}

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

# Persistent metadata is separate from send_watch.state so existing
# comma-separated condition state remains backward compatible.
function Read-Meta {
    $meta = @{
        capture_start_ts        = 0L
        capture_nonzero_samples = -1L
        capture_last_change_ts  = 0L
        capture_seen_audio      = $false
        drift_last_alert_ts     = 0L
        notified_active         = @()
    }
    if (-not (Test-Path $metaFile)) { return $meta }
    try {
        $saved = Get-Content $metaFile -Raw -ErrorAction Stop | ConvertFrom-Json
        if ($null -ne $saved.capture_start_ts) {
            $meta.capture_start_ts = [int64]$saved.capture_start_ts
        }
        if ($null -ne $saved.capture_nonzero_samples) {
            $meta.capture_nonzero_samples = [int64]$saved.capture_nonzero_samples
        }
        if ($null -ne $saved.capture_last_change_ts) {
            $meta.capture_last_change_ts = [int64]$saved.capture_last_change_ts
        }
        if ($null -ne $saved.capture_seen_audio) {
            $meta.capture_seen_audio = [bool]$saved.capture_seen_audio
        }
        if ($null -ne $saved.drift_last_alert_ts) {
            $meta.drift_last_alert_ts = [int64]$saved.drift_last_alert_ts
        }
        if ($null -ne $saved.notified_active) {
            $meta.notified_active = @($saved.notified_active | ForEach-Object { "$_" })
        }
    } catch {}
    return $meta
}

function Write-Meta([hashtable]$meta) {
    try {
        [ordered]@{
            capture_start_ts        = [int64]$meta.capture_start_ts
            capture_nonzero_samples = [int64]$meta.capture_nonzero_samples
            capture_last_change_ts  = [int64]$meta.capture_last_change_ts
            capture_seen_audio      = [bool]$meta.capture_seen_audio
            drift_last_alert_ts     = [int64]$meta.drift_last_alert_ts
            notified_active         = @($meta.notified_active)
        } | ConvertTo-Json -Compress | Set-Content -Path $metaFile -Encoding ascii
    } catch {}
}

# Read the tail of the JSONL log and extract the latest send_start cap
# and send_stats record. The last nonzero-sample change visible in the
# tail lets a newly installed watcher inherit up to TAIL_LINES of quiet
# history instead of starting its timer at zero. Returns a hashtable.
function Read-LogSnapshot {
    $snap = @{
        cap_ms                 = 1200
        start_ts               = 0L
        stats                  = $null
        nonzero_last_change_ts = 0L
        now                    = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
    }
    if (-not (Test-Path $Log)) { return $snap }
    $lines = Get-Content $Log -Tail $TAIL_LINES -ErrorAction SilentlyContinue
    $tailNonzero = -1L
    foreach ($line in $lines) {
        if ($line -notmatch '"event":"send_') { continue }
        try { $o = $line | ConvertFrom-Json } catch { continue }
        if ($o.event -eq 'send_start') {
            if ($o.capture_buffer_ms -gt 0) { $snap.cap_ms = [int]$o.capture_buffer_ms }
            $snap.start_ts = [int64]($o.ts_ms / 1000)
            $tailNonzero = -1L
            $snap.nonzero_last_change_ts = 0L
        } elseif ($o.event -eq 'send_stats') {
            $snap.stats = $o
            $nonzero = [int64]$o.capture_nonzero_samples
            if (
                ($tailNonzero -lt 0 -and $nonzero -gt 0) -or
                ($tailNonzero -ge 0 -and $nonzero -gt $tailNonzero)
            ) {
                $snap.nonzero_last_change_ts = [int64]($o.ts_ms / 1000)
            }
            $tailNonzero = $nonzero
        }
    }
    return $snap
}

# Update the persisted quiet-audio baseline and return quiet seconds.
# A sender restart resets whether this run has produced non-silent
# audio. Only a positive counter value or increase counts as audio;
# observing the initial zero merely starts the quiet timer.
function Update-CaptureState([hashtable]$snap, [hashtable]$meta) {
    if ($null -eq $snap.stats) { return 0L }

    $nonzero = [int64]$snap.stats.capture_nonzero_samples
    $startChanged = (
        $snap.start_ts -gt 0 -and
        $snap.start_ts -ne $meta.capture_start_ts
    )
    $counterReset = (
        $meta.capture_nonzero_samples -ge 0 -and
        $nonzero -lt $meta.capture_nonzero_samples
    )
    if ($startChanged -or $counterReset) {
        $meta.capture_seen_audio = $false
        if ($snap.start_ts -gt 0) {
            $meta.capture_last_change_ts = [int64]$snap.start_ts
        } else {
            $meta.capture_last_change_ts = [int64]$snap.now
        }
    }

    $counterChanged = (
        $meta.capture_nonzero_samples -lt 0 -or
        $nonzero -ne $meta.capture_nonzero_samples
    )
    if ($nonzero -gt 0 -and $counterChanged) {
        $changeTs = [int64]$snap.nonzero_last_change_ts
        if ($changeTs -le 0) { $changeTs = [int64]$snap.now }
        $meta.capture_last_change_ts = $changeTs
        $meta.capture_seen_audio = $true
    } elseif ($meta.capture_last_change_ts -le 0) {
        if ($snap.start_ts -gt 0) {
            $meta.capture_last_change_ts = [int64]$snap.start_ts
        } else {
            $meta.capture_last_change_ts = [int64]$snap.now
        }
    }

    if ($snap.start_ts -gt 0) { $meta.capture_start_ts = [int64]$snap.start_ts }
    $meta.capture_nonzero_samples = $nonzero
    return [Math]::Max(0L, ([int64]$snap.now - [int64]$meta.capture_last_change_ts))
}

# Evaluate the active trouble conditions for the snapshot. Returns a
# list of @{ name; detail } for each active condition.
function Get-Conditions(
    [hashtable]$snap,
    [hashtable]$meta,
    [long]$captureQuietSecs
) {
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
        $recentAudio = (
            $meta.capture_seen_audio -and
            $captureQuietSecs -lt $DriftQuietSuppressSecs
        )
        $list += @{
            name = 'drift'
            notify = $recentAudio
            detail = "send_drift_ppm=${ppm} saturated at +/-150 - stall/pause recovery (will settle)"
        }
    }
    $quietThresholdSecs = [int64]$CaptureQuietHours * 3600
    if (
        $age -le $STALE_SECS -and
        $CaptureQuietHours -gt 0 -and
        $captureQuietSecs -ge $quietThresholdSecs
    ) {
        $quietHours = [Math]::Round($captureQuietSecs / 3600.0, 1)
        $list += @{
            name = 'capture_quiet'
            detail = "capture audio quiet for ${quietHours} h: capture_nonzero_samples unchanged - normal if PC is idle; otherwise check default render endpoint/source app"
        }
    }
    return $list
}

function Get-RecoveryMessage([string]$name) {
    switch ($name) {
        'down'  { return "sender back up (send_stats resumed)" }
        'ring'  { return "capture ring back to normal" }
        'drift' { return "drift compensator unsaturating" }
        'capture_quiet' { return "capture quiet condition cleared (non-silent samples resumed or sender restarted)" }
    }
    return $null
}

function Send-ConditionAlert(
    [hashtable]$condition,
    [hashtable]$meta,
    [long]$now
) {
    $shouldAlert = ($null -eq $condition.notify -or [bool]$condition.notify)
    if ($shouldAlert -and $condition.name -eq 'drift' -and $DriftAlertCooldownMinutes -gt 0) {
        $cooldownSecs = [int64]$DriftAlertCooldownMinutes * 60
        $sinceLast = $now - [int64]$meta.drift_last_alert_ts
        if ($meta.drift_last_alert_ts -gt 0 -and $sinceLast -lt $cooldownSecs) {
            $shouldAlert = $false
        }
    }
    if (-not $shouldAlert) { return $false }

    Write-Alert $condition.detail
    if ($condition.name -eq 'drift') {
        $meta.drift_last_alert_ts = $now
    }
    return $true
}

# One-shot health check (scheduled task mode): alert on new problems,
# log recoveries, persist state so a problem alerts only once.
function Invoke-Check {
    Write-Heartbeat
    $metaExisted = Test-Path $metaFile
    $meta = Read-Meta
    $snap = Read-LogSnapshot
    $captureQuietSecs = Update-CaptureState $snap $meta
    $active = @(Get-Conditions $snap $meta $captureQuietSecs)

    $prev = @()
    if (Test-Path $stateFile) {
        $prev = @((Get-Content $stateFile -Raw).Trim() -split ',' | Where-Object { $_ })
    }
    $prevSet = @{}; foreach ($p in $prev) { $prevSet[$p] = $true }
    $activeSet = @{}; foreach ($a in $active) { $activeSet[$a.name] = $true }
    $prevNotifiedSet = @{}; foreach ($p in @($meta.notified_active)) { $prevNotifiedSet[$p] = $true }
    if (-not $metaExisted) {
        # Before metadata existed, every condition in send_watch.state
        # had already produced an alert.
        foreach ($p in $prev) { $prevNotifiedSet[$p] = $true }
    }
    $notifiedSet = @{}

    foreach ($c in $active) {
        if ($prevSet.ContainsKey($c.name) -and $prevNotifiedSet.ContainsKey($c.name)) {
            $notifiedSet[$c.name] = $true
            continue
        }

        if (Send-ConditionAlert $c $meta $snap.now) {
            $notifiedSet[$c.name] = $true
        }
    }
    foreach ($p in $prev) {
        if (-not $activeSet.ContainsKey($p) -and $prevNotifiedSet.ContainsKey($p)) {
            $msg = Get-RecoveryMessage $p
            if ($msg) { Write-Recovery $msg }
        }
    }

    $state = ($active | ForEach-Object { $_.name }) -join ','
    Set-Content -Path $stateFile -Value $state -Encoding ascii
    $meta.notified_active = @($notifiedSet.Keys)
    Write-Meta $meta
}

# Interactive foreground watcher: poll every $POLL_SECS, alert on
# transitions, track alarms in memory.
function Invoke-Watch {
    if (-not (Test-Path $Log)) {
        Write-Host "send_watch: log not found: $Log" -ForegroundColor Red
        exit 1
    }
    Write-Heartbeat
    $meta = Read-Meta
    $meta.notified_active = @()
    $prev = @{}
    $notified = @{}
    while ($true) {
        $snap = Read-LogSnapshot
        $captureQuietSecs = Update-CaptureState $snap $meta
        $active = @(Get-Conditions $snap $meta $captureQuietSecs)
        $activeSet = @{}; foreach ($a in $active) { $activeSet[$a.name] = $a }

        foreach ($c in $active) {
            if ($prev.ContainsKey($c.name) -and $notified.ContainsKey($c.name)) { continue }
            if (Send-ConditionAlert $c $meta $snap.now) {
                $notified[$c.name] = $true
            }
        }
        foreach ($k in $prev.Keys) {
            if (-not $activeSet.ContainsKey($k) -and $notified.ContainsKey($k)) {
                $msg = Get-RecoveryMessage $k
                if ($msg) { Write-Recovery $msg }
                $notified.Remove($k)
            }
        }
        $prev = $activeSet
        $meta.notified_active = @($notified.Keys)
        Write-Meta $meta
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
