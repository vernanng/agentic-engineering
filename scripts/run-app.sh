#!/usr/bin/env bash
# launchd entry point: load .env, run the current release binary.
cd "$(dirname "$0")/.."
set -a; [ -f .env ] && . ./.env; set +a
export PORT="${PORT:-8000}" LOOP_DB="${LOOP_DB:-$PWD/loop.db}"
exec "$PWD/releases/loop-app-latest"
