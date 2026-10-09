#!/usr/bin/env bash
# One command to run Sentinel for a demo: builds what's missing, starts the API and the
# dashboard (one process, one port), waits until it is healthy and prints a pre-flight checklist.
#
#   ./scripts/demo.sh            fresh database, showcase set of 15 programs
#   ./scripts/demo.sh --keep     keep the previous database (incidents, rules, watchlists)
#
# Ctrl+C stops it. Needs .env with YELLOWSTONE_ENDPOINT, YELLOWSTONE_TOKEN and SOLANA_RPC_URL.

set -euo pipefail
cd "$(dirname "$0")/.."

PORT="${SENTINEL_PORT:-8080}"
URL="http://localhost:${PORT}"
DB="${SENTINEL_DB:-sentinel-demo.db}"
KEEP=0
[ "${1:-}" = "--keep" ] && KEEP=1

say() { printf '\n\033[1m%s\033[0m\n' "$*"; }
fail() { printf '\n\033[31m%s\033[0m\n' "$*" >&2; exit 1; }

# ---- 1. config ---------------------------------------------------------------------------
[ -f .env ] || fail "No .env file. Copy .env.example to .env and add your Solami keys."
set -a; . ./.env; set +a
for v in YELLOWSTONE_ENDPOINT YELLOWSTONE_TOKEN SOLANA_RPC_URL; do
  [ -n "${!v:-}" ] || fail "$v is not set in .env"
done
if curl -s -m 2 -o /dev/null "$URL/api/status" 2>/dev/null; then
  fail "Something is already listening on port $PORT. Stop it first (pkill -f target/release/sentinel)."
fi

# ---- 2. dashboard build (only when missing or older than the source) ------------------------
say "1/3  Dashboard"
if [ ! -f web/dist/index.html ] || [ -n "$(find web/src web/index.html web/package.json -newer web/dist/index.html 2>/dev/null | head -1)" ]; then
  (cd web && { [ -d node_modules ] || npm ci --silent; } && npm run build --silent)
  echo "built web/dist"
else
  echo "web/dist is up to date"
fi

# ---- 3. backend build --------------------------------------------------------------------
say "2/3  Sentinel (release build; the first one takes a few minutes)"
cargo build --release --bin sentinel --quiet

# ---- 4. run ------------------------------------------------------------------------------
say "3/3  Starting on the 15-program showcase set"
export SENTINEL_PROGRAMS="$(tr -d '[:space:]' < scripts/showcase.txt)"
export SENTINEL_MAX_WATCHED="${SENTINEL_MAX_WATCHED:-40}"
export SENTINEL_PUBLIC_URL="${SENTINEL_PUBLIC_URL:-$URL}"
export SENTINEL_DB="$DB"
if [ "$KEEP" -eq 0 ]; then rm -f "$DB" "$DB-wal" "$DB-shm"; fi

LOG="$(mktemp -t sentinel-demo.XXXXXX)"
./target/release/sentinel >"$LOG" 2>&1 &
PID=$!
trap 'kill $PID 2>/dev/null; wait $PID 2>/dev/null; echo; echo "Stopped."' EXIT INT TERM

for _ in $(seq 1 60); do
  curl -s -m 2 -o /dev/null "$URL/api/status" && break
  kill -0 $PID 2>/dev/null || { cat "$LOG"; fail "Sentinel exited. See the log above."; }
  sleep 1
done

# Give the stream a moment, then report what a viewer would see.
sleep 12
python3 - "$URL" <<'PY'
import json, sys, urllib.request
base = sys.argv[1]
d = json.load(urllib.request.urlopen(base + "/api/status"))
s, b = d["stream"], d["pricing"]
behind = s.get("behind_chain_slots")
ok = lambda c: "\033[32m OK \033[0m" if c else "\033[31mFAIL\033[0m"
print(f"\n  [{ok(s['connected'] and not s.get('stalled'))}] Solami gRPC streaming   {s['ingest_tps']:.0f} tx/s")
print(f"  [{ok(behind is not None and behind * 0.4 <= 2)}] Behind the chain       {'unknown yet' if behind is None else f'{behind * 0.4:.1f}s'}")
print(f"  [{ok(b['enabled'] and not b['last_error'])}] Blur prices            {b['priced_mints']} tokens" + (f"  ({b['last_error']})" if b['last_error'] else ""))
print(f"  [{ok(len(d['programs']) == 15)}] Programs monitored     {len(d['programs'])} of 15")
PY

cat <<MSG

  Open        $URL
  API         $URL/api/status   (also /api/solami, /api/incidents, /api/catalog)
  Log         tail -f $LOG
  Pick txs    python3 scripts/demo_picks.py

  Detectors arm about 5 minutes after start. Wait until then before recording.
  Press Ctrl+C here to stop.

MSG
wait $PID
