#!/usr/bin/env bash
# VM-side deploy, called by cd.yml over SSH after `git pull` (and for the
# first manual install). Gate -> backup -> build -> swap -> health-check,
# roll back to the previous binary on failure (DB is not restored).
set -euo pipefail
cd "$(dirname "$0")/.."
bash scripts/merge-env.sh # GitHub secrets (.env.cd) -> .env, if synced
SHA=$(git rev-parse --short HEAD)

echo "==> gate: cargo test"
cargo test --release --locked --test gate 2>&1 | tail -n 3

echo "==> backup db"
mkdir -p backups releases
[ -f loop.db ] && cp loop.db "backups/loop-$(date +%Y%m%d-%H%M%S).db" || true

echo "==> build loop-app:$SHA"
cargo build --release 2>&1 | tail -n 1
# cp+mv: a same-SHA redeploy cannot cp over the running binary (text busy).
cp target/release/loop-app "releases/.tmp-loop-app-$SHA" && mv "releases/.tmp-loop-app-$SHA" "releases/loop-app-$SHA"
cp target/release/client "releases/.tmp-client-$SHA" 2>/dev/null && mv "releases/.tmp-client-$SHA" "releases/client-$SHA" || true

PREV=$(cat .loop-app.current 2>/dev/null || true)

restart() { # $1 = binary path
  ln -sf "$1" releases/loop-app-latest
  echo "$1" > .loop-app.current
  systemctl restart loop-app
}

echo "==> deploy loop-app:$SHA"
restart "$PWD/releases/loop-app-$SHA"

echo "==> health check http://localhost:8000"
# .env may set LOOP_TOKEN; /api requires it when set (else health check 401s).
TOKEN=$(grep '^LOOP_TOKEN=' .env 2>/dev/null | cut -d= -f2- || true)
AUTH=(); [ -n "$TOKEN" ] && AUTH=(-H "X-Loop-Token: $TOKEN")
for _ in $(seq 1 20); do
  if curl -fsS "${AUTH[@]}" http://localhost:8000/api/task-types >/dev/null 2>&1; then
    echo "deploy: OK loop-app:$SHA at http://localhost:8000"
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
  systemctl stop loop-app || true
fi
exit 1
