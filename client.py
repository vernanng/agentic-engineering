"""Detached CLI client: the browser is one thin client, this is another.

Same /api/* capability list, no browser. Stdlib only.
Usage: python3 client.py [--json] status|feed|runs|send "..."|vote <id> up|down
Env: LOOP_URL (default http://localhost:8000), LOOP_TOKEN (if the API needs it).
"""
import json, os, sys, urllib.request, urllib.error

BASE = os.environ.get("LOOP_URL", "http://localhost:8000")
TOKEN = os.environ.get("LOOP_TOKEN", "")

def call(method, path, data=None):
    req = urllib.request.Request(BASE + path, method=method,
        data=json.dumps(data).encode() if data is not None else None,
        headers={"Content-Type": "application/json", **({"X-Loop-Token": TOKEN} if TOKEN else {})})
    try:
        with urllib.request.urlopen(req, timeout=10) as r:
            return json.loads(r.read() or b"{}")
    except urllib.error.HTTPError as e:
        print(f"error {e.code}: {e.read().decode()[:200]}", file=sys.stderr); sys.exit(1)

def main(argv):
    as_json = "--json" in argv
    args = [a for a in argv if a != "--json"]
    cmd = args[0] if args else "status"
    if cmd == "status":
        out = call("GET", "/api/executions")
    elif cmd == "feed":
        out = call("GET", "/api/items?status=presented")
    elif cmd == "runs":
        out = call("GET", "/api/runs")
    elif cmd == "send" and len(args) == 3:  # send <task_type_id> "<input>"
        out = call("POST", "/api/runs", {"task_type_id": int(args[1]), "input": args[2]})
    elif cmd == "vote" and len(args) == 3:
        out = call("POST", f"/api/items/{args[1]}/vote", {"vote": args[2]})
    else:
        print(__doc__, file=sys.stderr); return 2
    if as_json or not isinstance(out, (dict, list)):
        print(json.dumps(out, indent=1)); return 0
    if cmd == "status":
        print(f"agents working now: {out['agents_working_now']} (live runs {out['live_runs']} + enabled crons {out['enabled_crons']})")
        print(f"jev calls: {out['jev_calls']} (mock {out['jev_mock_calls']}) cost ${out['jev_cost']:.5f} · step tokens {out['step_tokens']}")
        print(f"runs: {out['runs_by_status']}")
    elif cmd == "feed":
        for i in out: print(f"#{i['id']} [H{i['hype']} F{i['facts']} S{i['signal']}] {i['title']}")
    elif cmd == "runs":
        for r in out: print(f"#{r['id']} task={r['task_type_id']} mode={r['mode']} {r['status']} {str(r['input'])[:60]}")
    else:
        print(json.dumps(out, indent=1))
    return 0

if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
