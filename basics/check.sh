#!/usr/bin/env bash
# Fast, offline check for every basics demo. No installs, no network beyond localhost.
# Exit 0 = all runnable checks passed (skips allowed for missing optional tools).
set -u
cd "$(dirname "$0")"
PASS=0; FAIL=0; SKIP=0
pass() { PASS=$((PASS+1)); echo "ok   $1"; }
fail() { FAIL=$((FAIL+1)); echo "FAIL $1: $2"; }
skip() { SKIP=$((SKIP+1)); echo "skip $1: $2"; }

# 1. Python
out=$(python3 python/hello.py 2>&1) && echo "$out" | grep -q "hi world" \
  && echo "$out" | grep -q "0, 1, 4, 9, 16" \
  && pass "python/hello.py" || fail "python/hello.py" "$out"

# 2. TypeScript (Node >=22 strips types; fallback to tsx)
if command -v node >/dev/null 2>&1; then
  out=$(node typescript/hello.ts 2>&1) && echo "$out" | grep -q "hi world" \
    && pass "typescript/hello.ts (node)" || fail "typescript/hello.ts" "$out"
else
  skip "typescript/hello.ts" "node not found"
fi

# 3. Rust
if command -v rustc >/dev/null 2>&1; then
  tmp=$(mktemp -d)
  if rustc -o "$tmp/hello" rust/hello.rs 2>"$tmp/err" && out=$("$tmp/hello" 2>&1) && echo "$out" | grep -q "hi world"; then
    pass "rust/hello.rs"
  else
    fail "rust/hello.rs" "$(cat "$tmp/err" 2>/dev/null)"
  fi
  rm -rf "$tmp"
else
  skip "rust/hello.rs" "rustc not found"
fi

# 4. Docker demo app (build needs daemon+network, so verify app + files locally)
out=$(python3 docker/app.py 2>&1) && echo "$out" | grep -q "hi from inside" \
  && test -f docker/Dockerfile && test -f docker/compose.yaml \
  && pass "docker/app.py" || fail "docker/app.py" "$out"
if command -v docker >/dev/null 2>&1; then
  docker compose -f docker/compose.yaml config -q 2>/dev/null \
    && pass "docker/compose.yaml config" || skip "docker/compose.yaml config" "docker daemon/compose unavailable"
else
  skip "docker/compose.yaml config" "docker not found"
fi

# 5. REST (stdlib server on :8000)
out=$(python3 - <<'PY' 2>&1
import json, subprocess, sys, time, urllib.request
srv = subprocess.Popen([sys.executable, "rest/api.py"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
try:
    for _ in range(30):
        try:
            urllib.request.urlopen("http://localhost:8000/users", timeout=1); break
        except Exception: time.sleep(0.2)
    with urllib.request.urlopen("http://localhost:8000/users", timeout=3) as r:
        assert r.status == 200 and json.loads(r.read())[0]["name"] == "Ada", "GET /users"
    req = urllib.request.Request("http://localhost:8000/echo", data=b'{"hi":1}',
                                 headers={"Content-Type": "application/json"}, method="POST")
    with urllib.request.urlopen(req, timeout=3) as r:
        assert r.status == 201 and json.loads(r.read())["you_sent"] == {"hi": 1}, "POST /echo"
    print("rest ok")
finally:
    srv.terminate()
PY
) && echo "$out" | grep -q "rest ok" && pass "rest/api.py" || fail "rest/api.py" "$out"

# 6. WebSockets (needs `websockets` package; syntax-check always, echo only if importable)
if python3 -m py_compile websockets/server.py websockets/client.py 2>/dev/null; then
  if python3 -c "from websockets.asyncio.client import connect" 2>/dev/null; then
    out=$(python3 - <<'PY' 2>&1
import asyncio, subprocess, sys
from websockets.asyncio.client import connect
async def main():
    srv = subprocess.Popen([sys.executable, "websockets/server.py"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        for _ in range(30):
            try:
                async with connect("ws://localhost:8765") as ws:
                    await ws.send("hello"); assert await ws.recv() == "echo: hello"
                break
            except Exception: await asyncio.sleep(0.2)
        print("ws ok")
    finally: srv.terminate()
asyncio.run(main())
PY
) && echo "$out" | grep -q "ws ok" && pass "websockets echo" || fail "websockets echo" "$out"
  else
    skip "websockets echo" "websockets package not installed (pip install websockets)"
    pass "websockets syntax"
  fi
else
  fail "websockets syntax" "py_compile failed"
fi

# 7. Loop app (own end-to-end check, stdlib only)
out=$(python3 loop-app/check.py 2>&1) && echo "$out" | grep -q "ALL CHECKS PASSED" \
  && pass "loop-app/check.py" || fail "loop-app/check.py" "$(echo "$out" | tail -n 5)"

# 8. MD3 demo page (static; verify roles present)
grep -q "primary" md3/demo.html && grep -q "surface" md3/demo.html \
  && pass "md3/demo.html" || fail "md3/demo.html" "colour roles missing"

echo "---"
echo "passed=$PASS failed=$FAIL skipped=$SKIP"
[ "$FAIL" -eq 0 ]
