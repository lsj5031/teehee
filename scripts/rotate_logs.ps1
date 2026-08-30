<#
rotate_logs.ps1 - rotate teehee logs so send.jsonl cannot grow unbounded.

The sender appends to logs\send.jsonl forever; at ~150 MB per month it
will eat the disk. This script runs on its own schedule (TeeHeeRotate
scheduled task, daily) and:

  1. Rotates logs\send.jsonl when it exceeds MAX_SEND_MB (default 100):
     renames it to logs\archive\send-<UTC timestamp>.jsonl. The sender
     keeps writing to the open file handle, so rotation is safe while
     it runs; the new file is created on the next write.
  2. Rotates logs\send_alerts.log when it exceeds MAX_ALERT_MB
     (default 1) the same way.
  3. Prunes archives: keeps only KEEP_ARCHIVES (default 8) newest
     archived send-*.jsonl files, deletes older ones, and removes
     archive entries older than RETAIN_DAYS (default 90).

Safe to run while the sender is up: renaming an open file is atomic on
NTFS, and the sender recreates the file on its next write.

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
    Move-Item -Path $path -Destination $target -Force
    $displayName = if ((Get-Item $target -ErrorAction SilentlyContinue)) { (Get-Item $target).Name } else { $archivePrefix }
    Write-Host "[ROTATE] $displayName $([math]::Round($size/1MB,1)) MB -> $target"
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
