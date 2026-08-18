#!/usr/bin/env bash
# send_supervisor.sh — start the teehee sender and keep it alive.
#
# Runs the sender with the deployed flags (see SENDER_ARGS below).
# If the process exits — crash, manual kill, audio-device hiccup —
# it is restarted after a short delay with exponential backoff
# (2s, 4s, 8s, … capped at 60s) so a crash loop cannot spin. Every
# start/exit/restart event is appended to logs/supervisor.log.
#
# The supervisor enforces a single sender instance: before every
# start it kills any teehee.exe already running (a manual launch, a
# re-enabled task, or another supervisor's sender). A duplicate sender
# would double the audio stream and corrupt send.jsonl.
#
# The supervisor is itself a singleton: if another supervisor is
# already running (e.g. started manually, then the logon task fires),
# the newcomer exits so two supervisors cannot fight over the sender.
#
# Designed to run at logon via the "TeeHeeSender" scheduled task:
#
#   schtasks /create /tn TeeHeeSender \
#     /tr "\"C:\Program Files\Git\usr\bin\bash.exe\" T:\TeeHee\scripts\send_supervisor.sh" \
#     /sc onlogon /f
#
# Target and log file can be overridden via environment variables so
# the task command line never needs quotes/edits:
#   TEEHEE_HOST            (default 192.168.1.111)
#   TEEHEE_PORT            (default 5000)
#   TEEHEE_CHUNK_MS        (default 3)
#   TEEHEE_CAPTURE_BUFFER_MS (default 400)
#   TEEHEE_LOG_FILE        (default <repo>/logs/send.jsonl)
#
# Usage:  ./send_supervisor.sh     (foreground; the scheduled task
#                                   runs this in the logon session)

set -u

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"

SENDER="$ROOT/target/release/teehee.exe"
SUPERVISOR_LOG="$ROOT/logs/supervisor.log"
PID_FILE="$ROOT/logs/supervisor.pid"

HOST="${TEEHEE_HOST:-192.168.1.111}"
PORT="${TEEHEE_PORT:-5000}"
CHUNK_MS="${TEEHEE_CHUNK_MS:-3}"
CAPTURE_BUFFER_MS="${TEEHEE_CAPTURE_BUFFER_MS:-400}"
LOG_FILE="${TEEHEE_LOG_FILE:-$ROOT/logs/send.jsonl}"

SENDER_ARGS=(send --host "$HOST" --port "$PORT" --capture-source=loopback \
    --stats --chunk-ms "$CHUNK_MS" --log-file "$LOG_FILE" \
    --capture-buffer-ms "$CAPTURE_BUFFER_MS")

log() { echo "$(date '+%F %T') $*" >> "$SUPERVISOR_LOG"; }

# Refuse to run alongside another supervisor. Two supervisors would
# each restart the other's sender, recreating a duplicate stream.
# Uses a PID file rather than command-line matching (which would also
# match this script's own measurement commands).
check_singleton() {
    if [ -f "$PID_FILE" ]; then
        local other
        other=$(tr -d '\r\n ' < "$PID_FILE")
        if [ -n "$other" ] && kill -0 "$other" 2>/dev/null; then
            echo "send_supervisor: another supervisor instance (pid $other) is already running; exiting" >&2
            log "another supervisor instance (pid $other) is already running; exiting"
            exit 0
        fi
        log "removing stale pid file (pid $other is gone)"
    fi
    printf '%s\n' "$$" > "$PID_FILE"
    trap 'rm -f "$PID_FILE"' EXIT
}

# Kill any teehee.exe already running so a duplicate stream can never
# happen. No-op when nothing is running.
kill_stale_senders() {
    local n
    n=$(tasklist 2>/dev/null | grep -ci 'teehee\.exe' || true)
    if [ "${n:-0}" -gt 0 ]; then
        log "found $n stale teehee.exe instance(s); killing"
        taskkill //IM teehee.exe //F >/dev/null 2>&1 || true
    fi
}

if [ ! -f "$SENDER" ]; then
    echo "send_supervisor: sender not found: $SENDER" >&2
    exit 1
fi

check_singleton

log "supervisor starting (host=$HOST port=$PORT chunk_ms=$CHUNK_MS capture_buffer_ms=$CAPTURE_BUFFER_MS log=$LOG_FILE)"

failures=0
while :; do
    kill_stale_senders
    log "starting sender"
    "$SENDER" "${SENDER_ARGS[@]}"
    rc=$?
    if [ "$rc" -eq 0 ]; then
        log "sender exited cleanly (rc=0); restarting"
        failures=0
    else
        failures=$((failures + 1))
        log "sender exited with rc=$rc (failure #$failures)"
    fi
    # Exponential backoff: 2s, 4s, 8s, … capped at 60s.
    delay=$(( 2 << (failures > 5 ? 5 : failures - 1) ))
    [ "$delay" -gt 60 ] && delay=60
    log "restarting in ${delay}s"
    sleep "$delay"
done
