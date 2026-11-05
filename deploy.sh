#!/usr/bin/env bash
# Local CD via Compose: check -> build -> up -> health-check -> rollback on failure.
set -euo pipefail
cd "$(dirname "$0")"

SHA=$(git rev-parse --short HEAD)

command -v docker >/dev/null || { echo "deploy: docker CLI not found" >&2; exit 1; }
docker info >/dev/null 2>&1 || { echo "deploy: Docker daemon not running (try: colima start)" >&2; exit 1; }
docker compose version >/dev/null 2>&1 || { echo "deploy: docker compose not available" >&2; exit 1; }
echo "deploy: docker context = $(docker context show)"

echo "==> gate: python3 check.py"
python3 check.py

# Previous image tag for rollback (e.g. loop-app:abc1234 -> abc1234), empty on first deploy.
PREV=$(docker inspect -f '{{.Config.Image}}' loop-app 2>/dev/null || true)
PREV_TAG=${PREV#loop-app:}
[ "$PREV" = "loop-app:$PREV_TAG" ] || PREV_TAG=""

echo "==> build loop-app:$SHA"
IMAGE_TAG=$SHA docker compose build
docker tag "loop-app:$SHA" loop-app:latest

# Adopt a pre-Compose `docker run` container of the same name (Compose won't replace foreign containers).
if docker inspect loop-app >/dev/null 2>&1; then
  PROJ=$(docker inspect -f '{{index .Config.Labels "com.docker.compose.project"}}' loop-app 2>/dev/null || true)
  if [ -z "$PROJ" ]; then echo "deploy: removing pre-Compose loop-app container"; docker rm -f loop-app >/dev/null; fi
fi

docker volume create loop-data >/dev/null

echo "==> deploy loop-app:$SHA"
IMAGE_TAG=$SHA docker compose up -d

echo "==> health check http://localhost:8000"
for i in $(seq 1 20); do
  if curl -fsS http://localhost:8000/api/task-types >/dev/null 2>&1; then
    echo "deploy: OK loop-app:$SHA at http://localhost:8000"
    exit 0
  fi
  sleep 0.5
done

echo "deploy: health check failed, logs:" >&2
docker compose logs --tail=20 2>&1 | tail -20 >&2 || true
if [ -n "$PREV_TAG" ] && [ "$PREV_TAG" != "$SHA" ] && [ "$PREV_TAG" != "latest" ]; then
  echo "deploy: rolling back to loop-app:$PREV_TAG" >&2
  IMAGE_TAG=$PREV_TAG docker compose up -d
else
  docker compose down >/dev/null 2>&1 || true
fi
exit 1
