#!/usr/bin/env bash
# send_watch.sh — tail the teehee sender JSONL log and alert on trouble.
#
# Watches `send_stats` records and alerts (console line + beep) on
# state transitions only, so it is safe to leave running for days:
#
#   1. capture_ring_ms exceeds 50% of the capture-buffer cap
#      → delay is building, or the encode loop is being starved
#        (CPU contention / stall). With the drift compensator this
#        should be rare; when it fires, check machine load.
#   2. send_drift_ppm saturates at the ±150 clamp
#      → stall or long-pause recovery in progress; it settles on
#        its own within seconds to minutes. Worth knowing about.
#   3. no send_stats line for > 20 s
#      → the sender exited or hung (the stats thread emits every
#        second while the process is alive).
#
# The ring cap is auto-detected from the most recent `send_start`
# event's `capture_buffer_ms` (falls back to 400, the deployed
# setting). Recovery is reported with `[OK]` lines.
#
# Usage:  ./send_watch.sh [LOG_FILE]     (default: logs/send.jsonl)
# Requires: jq, tail -F (Git Bash / MSYS2 on Windows).

set -u

LOG="${1:-logs/send.jsonl}"
RING_WARN_PCT=50
PPM_SAT=149.0
STALE_SECS=20
POLL_SECS=10

[ -f "$LOG" ] || { echo "send_watch: log not found: $LOG" >&2; exit 1; }

# Ring cap (ms) from the most recent send_start; fall back to the
# deployed --capture-buffer-ms value.
cap_ms=400
last_start=$(grep '"event":"send_start"' "$LOG" | tail -1)
if [ -n "$last_start" ]; then
    c=$(printf '%s' "$last_start" | jq -r '.capture_buffer_ms // empty' 2>/dev/null)
    case "$c" in
    ''|null) ;;
    *) [ "$c" -gt 0 ] 2>/dev/null && cap_ms=$c ;;
    esac
fi
ring_thresh=$(( cap_ms * RING_WARN_PCT / 100 ))

# Wall-clock time (epoch s) of the most recent send_stats, so a
# sender that is already dead at startup is still caught.
last_ts=$(grep '"event":"send_stats"' "$LOG" | tail -1 | jq -r '.ts_ms // empty' 2>/dev/null)
last_seen=0
case "$last_ts" in
''|null) ;;
*) last_seen=$(( last_ts / 1000 )) ;;
esac

ring_alarm=0
ppm_alarm=0
stale_alarm=0

echo "send_watch: watching $LOG — ring alarm >${ring_thresh}ms (${RING_WARN_PCT}% of ${cap_ms}ms cap), ppm alarm |x|≥${PPM_SAT}, stale alarm >${STALE_SECS}s" >&2

tail -F -n 0 "$LOG" | while :; do
    if IFS= read -r -t "$POLL_SECS" line; then
        ev=$(printf '%s' "$line" | jq -r '.event // empty' 2>/dev/null)
        case "$ev" in
        send_start)
            c=$(printf '%s' "$line" | jq -r '.capture_buffer_ms // empty' 2>/dev/null)
            case "$c" in
            ''|null) ;;
            *) if [ "$c" -gt 0 ] 2>/dev/null; then
                   cap_ms=$c
                   ring_thresh=$(( cap_ms * RING_WARN_PCT / 100 ))
                   echo "send_watch: sender restarted — ring cap ${cap_ms}ms (alarm >${ring_thresh}ms)" >&2
               fi ;;
            esac
            ;;
        send_stats)
            last_seen=$(date +%s)
            if [ "$stale_alarm" = 1 ]; then
                stale_alarm=0
                echo "[OK] $(date +%T) send_stats resumed"
            fi
            ring=$(printf '%s' "$line" | jq -r '.capture_ring_ms // 0' 2>/dev/null)
            if [ "${ring:-0}" -gt "$ring_thresh" ]; then
                if [ "$ring_alarm" = 0 ]; then
                    ring_alarm=1
                    printf '\a'
                    echo "[ALERT] $(date +%T) capture_ring_ms=${ring} exceeds ${ring_thresh}ms (${RING_WARN_PCT}% of ${cap_ms}ms cap) — delay building or encode loop starved"
                fi
            elif [ "$ring_alarm" = 1 ]; then
                ring_alarm=0
                echo "[OK] $(date +%T) capture ring back to ${ring}ms"
            fi
            if printf '%s' "$line" | jq -e "(.send_drift_ppm // 0) | abs >= $PPM_SAT" >/dev/null 2>&1; then
                ppm=$(printf '%s' "$line" | jq -r '.send_drift_ppm // 0' 2>/dev/null)
                if [ "$ppm_alarm" = 0 ]; then
                    ppm_alarm=1
                    printf '\a'
                    echo "[ALERT] $(date +%T) send_drift_ppm=${ppm} saturated at ±150 — stall/pause recovery in progress (will settle)"
                fi
            elif [ "$ppm_alarm" = 1 ]; then
                ppm_alarm=0
                echo "[OK] $(date +%T) drift compensator unsaturating"
            fi
            ;;
        esac
    else
        # Quiet period: check whether the sender has gone silent.
        if [ "$last_seen" -ne 0 ] && [ "$stale_alarm" = 0 ] \
            && [ $(( $(date +%s) - last_seen )) -ge "$STALE_SECS" ]; then
            stale_alarm=1
            printf '\a'
            echo "[ALERT] $(date +%T) no send_stats for $(( $(date +%s) - last_seen ))s — sender exited or hung (check: tasklist | grep teehee)"
        fi
        sleep 1
    fi
done
