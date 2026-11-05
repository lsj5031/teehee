<#
rotate_logs.ps1 - rotate teehee logs so send.jsonl cannot grow unbounded.

The sender appends to logs\send.jsonl forever; at ~150 MB per month it
will eat the disk. This script runs on its own schedule (TeeHeeRotate
scheduled task, daily) and:

  1. Rotates logs\send.jsonl when it exceeds MAX_SEND_MB (default 100):
     copies it to logs\archive\send-<UTC timestamp>.jsonl, then
     truncates the live file to zero. The sender holds a
     process-lifetime append handle on send.jsonl and never reopens by
     path, so a rename would send future writes into the archive and
     the live file would never be recreated. Copy-and-truncate keeps
     the sender writing to the same open handle at the live path while
     the archive preserves the rotated data.
  2. Rotates logs\send_alerts.log when it exceeds MAX_ALERT_MB
     (default 1) the same way. (Add-Content reopens the path each
     write, so rename would also work here, but copy-and-truncate is
     uniform and avoids any open-handle edge case.)
  3. Prunes archives: keeps only KEEP_ARCHIVES (default 8) newest
     archived send-*.jsonl files, deletes older ones, and removes
     archive entries older than RETAIN_DAYS (default 90).

Safe to run while the sender is up: copy-and-truncate does not close
or move the sender's open file handle — it copies the contents to the
archive, then truncates the live file in place.

Usage:
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\rotate_logs.ps1
#>
param(
    [int]$MaxSendMb = 100,
    [int]$MaxAlertMb = 1,
    [int]$KeepArchives = 8,
    [int]$RetainDays = 90
)

$ErrorActionPreference = 'Continue'

# --- paths -----------------------------------------------------------
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$root = Split-Path -Parent $here
$logsDir = Join-Path $root 'logs'
$archiveDir = Join-Path $logsDir 'archive'
$sendLog = Join-Path $logsDir 'send.jsonl'
$alertLog = Join-Path $logsDir 'send_alerts.log'

if (-not (Test-Path $archiveDir)) {
    New-Item -ItemType Directory -Path $archiveDir -Force | Out-Null
}

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'

function Rotate-Log([string]$path, [string]$archivePrefix, [long]$maxBytes) {
    if (-not (Test-Path $path)) { return }
    $size = (Get-Item $path).Length
    if ($size -lt $maxBytes) { return }
    $ext = if ($archivePrefix -eq 'send') { 'jsonl' } else { 'log' }
    $target = Join-Path $archiveDir "$archivePrefix-$stamp.$ext"
    # Copy-Item opens the source for read (sharing the sender's append
    # handle), then we truncate the live file in place. The sender's
    # open handle stays valid and keeps writing at the live path.
    # The copy MUST succeed before the live log is truncated; a failed
    # archive would otherwise destroy the only copy of the data.
    try {
        Copy-Item -Path $path -Destination $target -Force -ErrorAction Stop
    } catch {
        Remove-Item -LiteralPath $target -Force -ErrorAction SilentlyContinue
        Write-Host "[ROTATE] ERROR: could not archive $path - $($_.Exception.Message); live log left intact"
        return
    }
    $archived = Get-Item -LiteralPath $target -ErrorAction SilentlyContinue
    # The sender can append after the initial size check. A successful
    # copy may therefore be larger than that snapshot; only a shorter
    # archive indicates that the original contents were not preserved.
    if (-not $archived -or $archived.Length -lt $size) {
        Write-Host "[ROTATE] ERROR: archive verification failed for $target; live log left intact"
        return
    }
    # Truncate: open for write (not append) with ReadWrite share so it
    # does not conflict with the sender's open append handle, then close
    # immediately. The sender's handle is unaffected; its next write
    # appends to the now-empty file at the same path.
    try {
        $stream = [System.IO.File]::Open($path, 'Truncate', 'Write', 'ReadWrite')
        $stream.Close()
    } catch {
        Write-Host "[ROTATE] WARNING: could not truncate $path - $($_.Exception.Message); archive retained"
        return
    }
    Write-Host "[ROTATE] $($archived.Name) $([math]::Round($size/1MB,1)) MB -> $target"
}

# --- rotate ----------------------------------------------------------
Rotate-Log $sendLog 'send' ($MaxSendMb * 1MB)
Rotate-Log $alertLog 'alerts' ($MaxAlertMb * 1MB)

# --- prune archives --------------------------------------------------
$archives = @(Get-ChildItem -Path $archiveDir -Filter 'send-*.jsonl' -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime -Descending)
if ($archives.Count -gt $KeepArchives) {
    $archives | Select-Object -Skip $KeepArchives | ForEach-Object {
        Remove-Item $_.FullName -Force
        Write-Host "[PRUNE] deleted $($_.Name)"
    }
}
Get-ChildItem -Path $archiveDir -ErrorAction SilentlyContinue |
    Where-Object { $_.LastWriteTime -lt (Get-Date).AddDays(-$RetainDays) } |
    ForEach-Object {
        Remove-Item $_.FullName -Force
        Write-Host "[PRUNE] deleted $($_.Name) (older than $RetainDays days)"
    }
