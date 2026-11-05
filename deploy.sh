#!/usr/bin/env bash
# Local CD without Docker: gate -> release build -> swap binary -> health-check -> rollback.
set -euo pipefail
cd "$(dirname "$0")"

BRANCH=$(git rev-parse --abbrev-ref HEAD)
[ "$BRANCH" = "main" ] || { echo "deploy: only from main (on $BRANCH)" >&2; exit 1; }
SHA=$(git rev-parse --short HEAD)

echo "==> gate: cargo test"
cargo test --release 2>&1 | tail -n 3

echo "==> backup db"
mkdir -p backups releases
[ -f loop.db ] && cp loop.db "backups/loop-$(date +%Y%m%d-%H%M%S).db" || true

echo "==> build loop-app:$SHA"
cargo build --release 2>&1 | tail -n 1
cp target/release/loop-app "releases/loop-app-$SHA"
cp target/release/client "releases/client-$SHA" 2>/dev/null || true

PREV=$(cat .loop-app.current 2>/dev/null || true)

restart() { # $1 = binary path
  if [ -f .loop-app.pid ]; then kill "$(cat .loop-app.pid)" 2>/dev/null || true; sleep 1; fi
  set -a; [ -f .env ] && . ./.env; set +a
  PORT="${PORT:-8000}" LOOP_DB="${LOOP_DB:-$PWD/loop.db}" nohup "$1" >> loop-app.log 2>&1 &
  echo $! > .loop-app.pid
  echo "$1" > .loop-app.current
}

echo "==> deploy loop-app:$SHA"
restart "$PWD/releases/loop-app-$SHA"

echo "==> health check http://localhost:8000"
for i in $(seq 1 20); do
  if curl -fsS http://localhost:8000/api/task-types >/dev/null 2>&1; then
    echo "deploy: OK loop-app:$SHA at http://localhost:8000 (pid $(cat .loop-app.pid))"
    exit 0
  fi
  sleep 0.5
done

echo "deploy: health check failed, log tail:" >&2
tail -n 20 loop-app.log >&2 || true
if [ -n "$PREV" ] && [ -f "$PREV" ]; then
  echo "deploy: rolling back to $PREV" >&2
  restart "$PREV"
else
  [ -f .loop-app.pid ] && kill "$(cat .loop-app.pid)" 2>/dev/null || true
fi
exit 1
