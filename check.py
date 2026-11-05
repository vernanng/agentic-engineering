"""End-to-end check through the real REST surface. python3 check.py (starts its own server on 18099)."""
import json, os, subprocess, sys, tempfile, time, urllib.request, urllib.error
from unittest import mock

BASE = "http://localhost:18099"

def call(method, path, data=None, expect=None):
    req = urllib.request.Request(BASE + path, method=method,
                                 data=json.dumps(data).encode() if data is not None else None,
                                 headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=5) as r:
            out, code = json.loads(r.read() or b"{}"), r.status
    except urllib.error.HTTPError as e:
        out, code = json.loads(e.read() or b"{}"), e.code
    if expect is not None:
        assert code == expect, f"{method} {path} -> {code} {out}, expected {expect}"
    return out

def wait_run(rid, status, timeout=8):
    end = time.time() + timeout
    while time.time() < end:
        r = call("GET", f"/api/runs/{rid}")
        if r["status"] == status:
            return r
        time.sleep(0.15)
    raise AssertionError(f"run {rid} never reached {status}: {call('GET', f'/api/runs/{rid}')}")

class _FakeResp:
    def __init__(self, content): self._b = json.dumps({"choices": [{"message": {"content": content}}]}).encode()
    def __enter__(self): return self
    def __exit__(self, *a): pass
    def read(self): return self._b

def test_openrouter_mocked():
    """run_step() OpenRouter path with a fake HTTP layer — no network, no spend."""
    os.environ["LOOP_DB"] = tempfile.NamedTemporaryFile(suffix=".db", delete=False).name
    os.environ["OPENROUTER_API_KEY"] = "test-key"
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    import server
    seen = {}
    def fake(content):
        def _open(req, timeout=None):
            seen["model"] = json.loads(req.data)["model"]
            return _FakeResp(content)
        return _open
    good = json.dumps({"output": "real summary", "confidence": 0.9})
    with mock.patch.object(server.urllib.request, "urlopen", fake(good)):
        assert server.run_step("draft summary", "hello world", 0.95) == ("real summary", 0.9)
        assert seen["model"] == server.OPENROUTER_MODEL, seen
        server.run_step("triage ticket", "hello world", 0.55)  # low template conf escalates
        assert seen["model"] == server.OPENROUTER_ESCALATION_MODEL, seen
    with mock.patch.object(server.urllib.request, "urlopen", fake("not json")):
        try:
            server.run_step("draft summary", "hello", 0.95); raise AssertionError("expected RuntimeError")
        except RuntimeError:
            pass
    server.OPENROUTER_API_KEY = ""  # no key -> stub, no spend
    assert server.run_step("draft summary", "hello", 0.95)[1] == 0.95
    try:
        server.run_step("draft summary", "FAIL x", 0.95); raise AssertionError("expected RuntimeError")
    except RuntimeError:
        pass
    print("openrouter mock ok")

def main():
    types = {t["name"]: t for t in call("GET", "/api/task-types")}
    # 1. in-the-loop: pauses before each step, approve approves, reject kills
    tt = call("POST", "/api/task-types", {"name": "check-in", "template": "summarize", "mode": "in"}, 201)
    run = call("POST", "/api/runs", {"task_type_id": tt["id"], "input": "hello"}, 201)
    r = wait_run(run["id"], "awaiting_decision")
    assert r["steps"][0]["status"] == "awaiting_decision" and len(r["steps"]) == 1
    call("POST", f"/api/runs/{run['id']}/decide", {"action": "approve"}, 200)
    r = wait_run(run["id"], "awaiting_decision")
    call("POST", f"/api/runs/{run['id']}/decide", {"action": "approve", "edited_output": "human final"}, 200)
    r = wait_run(run["id"], "completed")
    assert r["steps"][1]["output"] == "human final", r["steps"]
    run = call("POST", "/api/runs", {"task_type_id": tt["id"], "input": "hello"}, 201)
    wait_run(run["id"], "awaiting_decision")
    call("POST", f"/api/runs/{run['id']}/decide", {"action": "reject"}, 200)
    wait_run(run["id"], "killed")
    print("in-the-loop ok")

    # 2. near-the-loop: clean run auto-completes, risky/low-confidence pauses
    tt2 = call("POST", "/api/task-types", {"name": "check-near", "template": "summarize", "mode": "near"}, 201)
    run = call("POST", "/api/runs", {"task_type_id": tt2["id"], "input": "clean"}, 201)
    wait_run(run["id"], "completed")
    tt3 = call("POST", "/api/task-types", {"name": "check-near2", "template": "classify", "mode": "near"}, 201)
    run = call("POST", "/api/runs", {"task_type_id": tt3["id"], "input": "ticket"}, 201)
    r = wait_run(run["id"], "awaiting_decision")
    assert "low confidence" in r["steps"][0]["pause_reason"], r["steps"]
    call("POST", f"/api/runs/{run['id']}/decide", {"action": "approve"}, 200)
    r = wait_run(run["id"], "awaiting_decision")
    assert "risky" in r["steps"][1]["pause_reason"], r["steps"]
    call("POST", f"/api/runs/{run['id']}/decide", {"action": "approve"}, 200)
    wait_run(run["id"], "completed")
    print("near-the-loop ok")

    # 3. out-of-the-loop: FAIL halts, alerts, demotes out -> near
    tt4 = call("POST", "/api/task-types", {"name": "check-out", "template": "summarize", "mode": "out"}, 201)
    run = call("POST", "/api/runs", {"task_type_id": tt4["id"], "input": "FAIL please"}, 201)
    r = wait_run(run["id"], "failed")
    assert any(a["event"] == "alert" for a in r["audit"]), r["audit"]
    after = [t for t in call("GET", "/api/task-types") if t["id"] == tt4["id"]][0]
    assert after["mode"] == "near", after
    print("out-of-the-loop ok")

    # 4. promotion: 3 clean in-loop runs earn near
    tt5 = call("POST", "/api/task-types", {"name": "check-promo", "template": "summarize", "mode": "in"}, 201)
    for _ in range(3):
        run = call("POST", "/api/runs", {"task_type_id": tt5["id"], "input": "ok"}, 201)
        for _ in range(2):
            wait_run(run["id"], "awaiting_decision")
            call("POST", f"/api/runs/{run['id']}/decide", {"action": "approve"}, 200)
        wait_run(run["id"], "completed")
    me = [t for t in call("GET", "/api/task-types") if t["id"] == tt5["id"]][0]
    assert me["promotion_eligible"], me
    promoted = call("POST", f"/api/task-types/{tt5['id']}/promote", {}, 200)
    assert promoted["mode"] == "near", promoted
    print("promotion ok")
    print("ALL CHECKS PASSED")

if __name__ == "__main__":
    test_openrouter_mocked()
    env = {k: v for k, v in os.environ.items() if k != "OPENROUTER_API_KEY"}  # gate never spends
    env.update({"PORT": "18099", "LOOP_DB": tempfile.NamedTemporaryFile(suffix=".db", delete=False).name})
    srv = subprocess.Popen([sys.executable, os.path.join(os.path.dirname(__file__), "server.py")], env=env)
    try:
        for _ in range(50):
            try:
                call("GET", "/api/task-types"); break
            except Exception:
                time.sleep(0.2)
        main()
    finally:
        srv.terminate()
