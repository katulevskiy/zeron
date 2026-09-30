#!/bin/bash
# Runs the native WebKit preview fixture with two guards against a flaky GitHub runner:
#  1. WebKit sometimes fails to resolve *.localhost (NSURLErrorDomain -1003) although the preview
#     proxy is listening; map the fixture hostnames to loopback in /etc/hosts so DNS never fails.
#     (The fixture still goes through the same per-domain proxy; this only removes the DNS race.)
#  2. Bounded retries (each attempt has its own timeout) instead of one 3 minute hang.
# usage: run-preview-fixture-macos.sh <fixture-binary> <capture-dir>
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BINARY="$1"
OUT="$2"
ATTEMPTS="${PREVIEW_FIXTURE_ATTEMPTS:-3}"
ATTEMPT_TIMEOUT="${PREVIEW_FIXTURE_ATTEMPT_TIMEOUT:-70}"

# Same slug() as crates/preview/src/catalog.rs: device label = slug(hostname).
label=$(hostname | python3 -c 'import re,sys; s=re.sub(r"[^a-z0-9]+","-",sys.stdin.read().strip().lower()).strip("-")[:48].strip("-"); print(s or "project")')
for svc in fieldnotes fieldnotes-vite fieldnotes-api fieldnotes-node fieldnotes-web fieldnotes-server; do
  echo "127.0.0.1 $label.$svc.localhost"
  echo "::1 $label.$svc.localhost"
done | sudo tee -a /etc/hosts >/dev/null
echo "hosts entries added for device label '$label'"

# macOS has no coreutils `timeout`; do it in bash.
run_with_timeout() {
  local secs=$1; shift
  "$@" &
  local pid=$!
  ( sleep "$secs"; kill -TERM "$pid" 2>/dev/null; sleep 3; kill -KILL "$pid" 2>/dev/null ) &
  local watchdog=$!
  wait "$pid"; local rc=$?
  kill "$watchdog" 2>/dev/null; wait "$watchdog" 2>/dev/null
  return $rc
}

for attempt in $(seq 1 "$ATTEMPTS"); do
  rm -rf "$OUT"; mkdir -p "$OUT"
  echo "=== preview fixture attempt $attempt/$ATTEMPTS ==="
  if run_with_timeout "$ATTEMPT_TIMEOUT" bash "$ROOT/scripts/run-macos-browser-fixture.sh" "$BINARY" "$OUT" && test -f "$OUT/result.txt"; then
    echo "preview fixture passed on attempt $attempt"
    exit 0
  fi
  echo "preview fixture attempt $attempt failed"
  dscacheutil -q host -a name "$label.fieldnotes.localhost" || true
  lsof -nP -iTCP:7331 -sTCP:LISTEN || true
  mkdir -p "$OUT.failed-$attempt"
  cp -R "$OUT"/. "$OUT.failed-$attempt"/ 2>/dev/null || true
  pkill -f 'vite/bin/vite.js' 2>/dev/null || true
  pkill -f 'node api.js' 2>/dev/null || true
  pkill -f 'Fixture.app/Contents/MacOS/fixture' 2>/dev/null || true
  sleep 2
done
echo "preview fixture failed after $ATTEMPTS attempts"
exit 1
