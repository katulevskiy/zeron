#!/usr/bin/env bash
# Times the Sessions <-> Settings tab switch of a debug build of the Android app.
#
#   scripts/android/measure-tab-switch.sh [-s SERIAL] [-n ROUNDS] [--ab] [--cold] [--launch]
#
# The app logs "ZeronPerf" lines (core/Perf.kt) for each interaction it is asked to time; this drives the
# switch through the debug broadcast (`--es kind tab --es tab sessions|settings`, see apps/android/README.md) and
# prints, per direction, the median of
#   cpu   main-thread CPU time from the request to the start of the second frame after it
#         (steady under host load, and what a faster phone shrinks)
#   wall  the same span in wall-clock time (includes waiting for the emulator's software renderer, so it is noisy)
#   ui    UI-thread time of the switch's first frame (input + recomposition + measure/layout + record)
#
#   --ab      also time the old behaviour (`retain=false`: the tab is rebuilt on every switch), interleaved with
#             the new one so both see the same host load
#   --cold    time rebuilding the whole home page for the target tab (what coming back from a chat costs)
#   --launch  start the app in demo mode first (`--ez demo true`); otherwise it must already be running
#
# Needs a debuggable build installed (`./gradlew :app:installDebug`). Never use screencap on the emulator.
set -euo pipefail

SERIAL="" ROUNDS=8 AB=0 COLD=0 LAUNCH=0
while [ $# -gt 0 ]; do
  case "$1" in
    -s) SERIAL="$2"; shift 2 ;;
    -n) ROUNDS="$2"; shift 2 ;;
    --ab) AB=1; shift ;;
    --cold) COLD=1; shift ;;
    --launch) LAUNCH=1; shift ;;
    -h|--help) sed -n '2,22p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

ADB=(adb)
[ -n "$SERIAL" ] && ADB=(adb -s "$SERIAL")
PKG=sh.zeron.android
broadcast() { "${ADB[@]}" shell am broadcast -a "$PKG.DEBUG_EVENT" -p "$PKG" --es kind tab "$@" >/dev/null; }

if [ "$LAUNCH" = 1 ]; then
  "${ADB[@]}" shell am start -W -n "$PKG/.MainActivity" --ez demo true >/dev/null
  sleep 12   # the engine, the demo workspace and the first idle composition of the other tab
fi
"${ADB[@]}" shell pidof "$PKG" >/dev/null || { echo "$PKG is not running (use --launch, or start it)" >&2; exit 1; }

LOG=$(mktemp)
"${ADB[@]}" logcat -c
modes=(true); [ "$AB" = 1 ] && modes=(false true)
for round in $(seq "$ROUNDS"); do
  for retain in "${modes[@]}"; do
    broadcast --es tab settings --ez retain "$retain" --ez warm true; sleep 1.5
    broadcast --es tab sessions --ez warm true; sleep 1.5
    broadcast --es tab settings --ez warm true; sleep 1.5
    for tab in sessions settings; do
      if [ "$COLD" = 1 ]; then broadcast --es tab "$tab" --ez cold true; else broadcast --es tab "$tab"; fi
      sleep 1.5
    done
  done
done
"${ADB[@]}" logcat -d -s ZeronPerf >"$LOG"

python3 - "$LOG" <<'PY'
import re, statistics as st, sys
rows = {}
for line in open(sys.argv[1]):
    m = re.search(r'(tab->\w+)((?: \[[^\]]+\])*): first frame after composition \+([\d.]+) ms wall, ([\d.]+) ms main-thread cpu', line)
    if m:
        tags = m.group(2)
        if 'warm-up' in tags:
            continue
        rows.setdefault((m.group(1), tags.strip() or '[retained tabs]'), {})['wall_cpu'] = rows.get((m.group(1), tags.strip() or '[retained tabs]'), {}).get('wall_cpu', []) + [(float(m.group(3)), float(m.group(4)))]
        last = (m.group(1), tags.strip() or '[retained tabs]')
        continue
    m = re.search(r'(tab->\w+)((?: \[[^\]]+\])*): frames=\d+ main-thread ms per frame \[([^\]]*)\]', line)
    if m and 'warm-up' not in m.group(2):
        key = (m.group(1), m.group(2).strip() or '[retained tabs]')
        first = [float(x) for x in m.group(3).split('|')][0]
        rows.setdefault(key, {}).setdefault('ui', []).append(first)
print('%-16s %-18s %3s  %12s  %12s  %12s' % ('switch', 'mode', 'n', 'cpu ms', 'wall ms', 'ui ms'))
for key in sorted(rows):
    r = rows[key]
    wc = r.get('wall_cpu', [])
    if not wc:
        continue
    cpu = [c for w, c in wc]
    wall = [w for w, c in wc]
    ui = r.get('ui', [0.0])
    print('%-16s %-18s %3d  %5.0f (%3.0f-%3.0f)  %5.0f        %5.0f' % (key[0], key[1], len(wc), st.median(cpu), min(cpu), max(cpu), st.median(wall), st.median(ui)))
PY
rm -f "$LOG"
