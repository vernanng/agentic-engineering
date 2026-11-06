#!/usr/bin/env bash
# Self-check for scripts/merge-env.sh: replacement, append, empty-skip,
# unmanaged keys untouched, .env.cd removed. Run: bash scripts/test-merge-env.sh
set -euo pipefail
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
mkdir "$T/scripts"
cp "$(dirname "$0")/merge-env.sh" "$T/scripts/"

cat > "$T/.env" <<'EOF'
OPENROUTER_API_KEY=keep-me
LOOP_TOKEN=old-token
HOST=0.0.0.0
EOF
cat > "$T/.env.cd" <<'EOF'
LOOP_TOKEN=new-token
OPENROUTER_API_BASE=https://openrouter.ai
TURSO_DATABASE_URL=
TURSO_AUTH_TOKEN=tok123
EOF

(cd "$T" && bash scripts/merge-env.sh)

grep -qx 'LOOP_TOKEN=new-token' "$T/.env"
grep -qx 'OPENROUTER_API_BASE=https://openrouter.ai' "$T/.env"
grep -qx 'TURSO_AUTH_TOKEN=tok123' "$T/.env"
grep -qx 'OPENROUTER_API_KEY=keep-me' "$T/.env"
grep -qx 'HOST=0.0.0.0' "$T/.env"
! grep -q '^TURSO_DATABASE_URL=' "$T/.env" # empty value skipped, not appended
[ ! -e "$T/.env.cd" ]
echo "test-merge-env: ok"
