#!/usr/bin/env bash
# Merge GitHub-synced secrets (.env.cd, piped in by cd.yml) into .env.
# Only the 4 managed keys, only non-empty values; other .env keys untouched.
set -euo pipefail
cd "$(dirname "$0")/.."
[ -f .env.cd ] || exit 0
touch .env
for k in LOOP_TOKEN OPENROUTER_API_BASE TURSO_DATABASE_URL TURSO_AUTH_TOKEN; do
  v=$(grep "^$k=" .env.cd | cut -d= -f2- || true)
  [ -z "$v" ] && continue
  grep -v "^$k=" .env > .env.tmp || true
  printf '%s=%s\n' "$k" "$v" >> .env.tmp
  chmod 600 .env.tmp
  mv .env.tmp .env
done
rm -f .env.cd
