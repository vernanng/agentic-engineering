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

def test_jev_wire_and_resume():
    """Real-Jev wire shape with a fake HTTP layer + queued-run resume. No spend."""
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    import server
    server.init_db()
    server.OPENROUTER_API_KEY = "test-key"
    seen = {}
    class _Resp:
        def __enter__(self): return self
        def __exit__(self, *a): pass
        def read(self): return json.dumps({"model": "jev-test",
            "answers": {"worth": {"type": "noul", "noul": 0.9}},
            "usage": {"input_tokens": 1000}}).encode()
    def fake_open(req, timeout=None):
        seen["url"] = req.full_url
        seen["model"] = json.loads(req.data)["model"]
        return _Resp()
    with mock.patch.object(server.urllib.request, "urlopen", fake_open):
        out = server.jev_classify({"text": "x"}, {"worth": {"type": "noul", "instructions": "worth it?"}})
        assert out["worth"]["noul"] == 0.9 and seen["url"].endswith("/api/alpha/decisions"), seen
        assert seen["model"] == "~typesafe/jev-latest", seen
    calls = {"n": 0}
    def flaky_open(req, timeout=None):
        calls["n"] += 1
        if calls["n"] == 1:
            raise urllib.error.HTTPError(req.full_url, 503, "busy", {}, None)
        return _Resp()
    with mock.patch.object(server.urllib.request, "urlopen", flaky_open):
        assert server.jev_classify({"text": "x"}, {"worth": {"type": "noul", "instructions": "?"}})["worth"]["noul"] == 0.9
        assert calls["n"] == 2, calls
    saved = server.OPENROUTER_API_KEY
    server.OPENROUTER_API_KEY = ""  # fail closed, never silently fake
    try:
        server.jev_classify({"text": "x"}, {"worth": {"type": "noul", "instructions": "?"}}); raise AssertionError("expected RuntimeError")
    except RuntimeError:
        pass
    finally:
        server.OPENROUTER_API_KEY = saved
    # resume: a queued run starts on boot; done steps are checkpoints (none here)
    server.init_db()
    tt = server.q("SELECT * FROM task_types WHERE name='summarize'", one=True)
    rid = server.x("INSERT INTO runs(task_type_id,input,mode,status,created_at) VALUES(?,?,?,'queued',?)",
                   (tt["id"], "resume me", tt["mode"], time.time()))
    server.resume_queued()
    end = time.time() + 8
    while time.time() < end:
        if server.q("SELECT status FROM runs WHERE id=?", (rid,), one=True)["status"] == "awaiting_decision": break
        time.sleep(0.15)
    assert server.q("SELECT status FROM runs WHERE id=?", (rid,), one=True)["status"] == "awaiting_decision"
    print("jev wire + resume ok")

def main():
    types = {t["name"]: t for t in call("GET", "/api/task-types")}
    call("POST", "/api/task-types", {"name": "check-badmode", "template": "summarize", "mode": "bogus"}, 400)
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

    # 5. delete: finished runs only; deleted run is gone
    tt6 = call("POST", "/api/task-types", {"name": "check-del", "template": "summarize", "mode": "in"}, 201)
    done = call("POST", "/api/runs", {"task_type_id": tt6["id"], "input": "ok"}, 201)
    for _ in range(2):
        wait_run(done["id"], "awaiting_decision")
        call("POST", f"/api/runs/{done['id']}/decide", {"action": "approve"}, 200)
    wait_run(done["id"], "completed")
    live = call("POST", "/api/runs", {"task_type_id": tt6["id"], "input": "ok"}, 201)
    wait_run(live["id"], "awaiting_decision")
    call("DELETE", f"/api/runs/{live['id']}", None, 400)
    call("POST", f"/api/runs/{live['id']}/decide", {"action": "reject"}, 200)
    wait_run(live["id"], "killed")
    call("DELETE", f"/api/runs/{done['id']}", None, 200)
    call("GET", f"/api/runs/{done['id']}", None, 404)
    call("DELETE", f"/api/runs/{done['id']}", None, 404)
    print("delete ok")

    # 6. bad input is a 400, never a dropped connection or a stranded run
    call("POST", "/api/task-types", {"name": 123, "template": "summarize"}, 400)
    call("POST", "/api/task-types", {"name": None, "template": "summarize"}, 400)
    n_before = len(call("GET", "/api/runs"))
    call("POST", "/api/runs", {"task_type_id": tt6["id"], "input": None}, 400)
    call("POST", "/api/runs", {"task_type_id": tt6["id"], "input": {"k": 1}}, 400)
    assert len(call("GET", "/api/runs")) == n_before, "rejected runs must not be created"
    # POST routing ignores the query string, like GET/DELETE do
    run = call("POST", "/api/runs?x=1", {"task_type_id": tt6["id"], "input": "ok"}, 201)
    for _ in range(2):
        wait_run(run["id"], "awaiting_decision")
        call("POST", f"/api/runs/{run['id']}/decide?x=1", {"action": "approve"}, 200)
    wait_run(run["id"], "completed")
    print("validation ok")

    # 7. Tiny Software Factory clone (docs/tsf-clone.md)
    manifest = call("GET", "/api/tools")
    names = {t["name"] for t in manifest["tools"]}
    assert {"start_run", "decide", "create_cron", "create_page", "propose_config", "create_factory"} <= names, names
    assert set(manifest["slices"]) == {"factory", "coding", "pages", "factory-controls"}
    cfg = call("GET", "/api/config")
    assert cfg["thinking_level"] in ("low", "medium", "high", "max") and cfg["enabled_slices"]
    call("PUT", "/api/config", {"bogus_key": 1}, 400)
    call("PUT", "/api/config", {"thinking_level": "high"}, 200)
    assert call("GET", "/api/config")["thinking_level"] == "high"
    print("tools+config ok")

    page = call("POST", "/api/pages", {"title": "Check Page", "body": "<b>hi</b>", "created_by": "agent"}, 201)
    assert any(p["id"] == page["id"] for p in call("GET", "/api/pages"))
    edited = call("POST", f"/api/pages/{page['id']}", {"body": "<i>edited</i>"}, 200)
    assert edited["body"] == "<i>edited</i>"
    call("DELETE", f"/api/pages/{page['id']}", None, 200)
    call("GET", f"/api/pages/{page['id']}", None, 404)
    print("pages ok")

    # self-modification: nothing applies until a human approves; out cannot propose
    prop = call("POST", "/api/proposals", {"task_type_id": tt["id"], "config": {"thinking_level": "max"}}, 201)
    assert call("GET", "/api/config")["thinking_level"] == "high", "proposal must not self-apply"
    call("POST", f"/api/proposals/{prop['id']}/decide", {"action": "approve"}, 200)
    assert call("GET", "/api/config")["thinking_level"] == "max"
    call("POST", f"/api/proposals/{prop['id']}/decide", {"action": "approve"}, 400)
    tt_out = call("POST", "/api/task-types", {"name": "check-out2", "template": "summarize", "mode": "out"}, 201)
    call("POST", "/api/proposals", {"task_type_id": tt_out["id"], "config": {"thinking_level": "low"}}, 400)
    print("proposals ok")

    # disabling the coding slice stops execution (pause/alert path, not a crash)
    call("PUT", "/api/config", {"enabled_slices": ["factory", "pages", "factory-controls"]}, 200)
    run = call("POST", "/api/runs", {"task_type_id": tt2["id"], "input": "slice check"}, 201)
    r = wait_run(run["id"], "awaiting_decision")
    assert "coding slice disabled" in r["steps"][0]["pause_reason"], r["steps"]
    call("POST", f"/api/runs/{run['id']}/decide", {"action": "reject"}, 200)
    wait_run(run["id"], "failed")
    call("PUT", "/api/config", {"enabled_slices": ["factory", "coding", "pages", "factory-controls"]}, 200)
    print("slices ok")

    # cron prompts: fires with nobody watching, creates a real audited run
    ttc = call("POST", "/api/task-types", {"name": "check-cron", "template": "summarize", "mode": "near"}, 201)
    cron = call("POST", "/api/crons", {"name": "check-tick", "schedule_s": 1, "prompt": "cron hello", "task_type_id": ttc["id"]}, 201)
    end = time.time() + 12
    fired = None
    while time.time() < end and fired is None:
        fired = next((a for a in call("GET", "/api/audit") if a["event"] == "cron_fired" and f"cron {cron['id']}" in (a["detail"] or "")), None)
        if fired is None: time.sleep(0.3)
    assert fired is not None, "cron never fired"
    cron_run = call("GET", f"/api/runs/{fired['run_id']}")
    assert cron_run["task_type_id"] == ttc["id"] and cron_run["input"] == "cron hello", cron_run
    wait_run(cron_run["id"], "completed")
    call("DELETE", f"/api/crons/{cron['id']}", None, 200)
    print("cron ok")

    # initialize/configure/specialize in one call (API + CLI share create_factory)
    fac = call("POST", "/api/factories", {"name": "check-factory", "template": "summarize",
        "crons": [{"name": "f-cron", "schedule_s": 3600, "prompt": "hello"}],
        "starter_page": {"title": "Start", "body": "<b>hi</b>"}}, 201)
    assert fac["task_type"]["mode"] == "in" and len(fac["crons"]) == 1 and fac["page"]["title"] == "Start", fac
    call("DELETE", f"/api/crons/{fac['crons'][0]['id']}", None, 200)
    with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as f:
        json.dump({"name": "check-factory-cli", "template": "report"}, f); cli_cfg = f.name
    out = subprocess.run([sys.executable, os.path.join(os.path.dirname(os.path.abspath(__file__)), "server.py"),
                          "--init-factory", cli_cfg],
                         env={**os.environ, "LOOP_DB": tempfile.NamedTemporaryFile(suffix=".db", delete=False).name},
                         capture_output=True, text=True, timeout=15)
    assert out.returncode == 0 and json.loads(out.stdout)["task_type"]["mode"] == "in", out.stderr
    print("factory init ok")

    # 8. Leaving the loop: triggers, real-Jev sweep (labelled mock), flywheel, incident, CLI
    run = call("POST", f"/api/triggers/{tt2['id']}", {"input": "via webhook"}, 201)
    r = wait_run(run["id"], "completed")
    assert any(a["event"] == "trigger_fired" and "webhook" in (a["detail"] or "") for a in r["audit"]), r["audit"]
    print("webhook trigger ok")

    tti = call("POST", "/api/task-types", {"name": "check-incident", "template": "incident", "mode": "near"}, 201)
    ev = call("POST", "/api/events", {"status": 503, "message": "db pool exhausted", "task_type_id": tti["id"]}, 201)
    assert ev["started"] is True, ev
    r = wait_run(ev["run"]["id"], "awaiting_decision")
    assert r["steps"][-1]["name"] == "apply fix" and "risky" in (r["steps"][-1]["pause_reason"] or ""), r["steps"]
    call("POST", f"/api/runs/{ev['run']['id']}/decide", {"action": "approve"}, 200)
    wait_run(ev["run"]["id"], "completed")
    ev2 = call("POST", "/api/events", {"status": 200, "message": "routine health ping", "task_type_id": tti["id"]}, 200)
    assert ev2["started"] is False, ev2  # Jev relevance gates it, no incident started
    print("incident ok")

    ttn = call("POST", "/api/task-types", {"name": "check-news", "template": "news", "mode": "near"}, 201)
    good = call("POST", "/api/items", {"title": "Agent framework release v2 available now",
        "body": "The agent model release v2 is available now with a new flag.", "source": "wire-a"}, 201)
    call("POST", "/api/items", {"title": "Agent framework release v2 available now",
        "body": "The agent model release v2 is available now with a new flag.", "source": "wire-a"}, 200)  # hash dupe folds at ingest
    bad = call("POST", "/api/items", {"title": "Celebrity giveaway scandal shocks fans",
        "body": "A celebrity giveaway scandal with no engineering content.", "source": "wire-b"}, 201)
    run = call("POST", "/api/runs", {"task_type_id": ttn["id"], "input": "sweep"}, 201)
    wait_run(run["id"], "completed")
    items = {i["id"]: i for i in call("GET", "/api/items")}
    assert items[good["id"]]["status"] == "presented" and items[good["id"]]["signal"] is not None, items[good["id"]]
    assert items[bad["id"]]["status"] == "gated", items[bad["id"]]  # Jev relevance stopped it before any card
    assert len(call("GET", "/api/items?status=presented")) <= 7
    ex = call("GET", "/api/executions")
    assert ex["jev_calls"] > 0 and ex["jev_mock_calls"] == ex["jev_calls"], ex  # gate uses the labelled mock only
    print("news sweep ok")

    before = {w["key"]: w["weight"] for w in call("GET", "/api/weights") if w["kind"] == "source"}
    call("POST", f"/api/items/{good['id']}/vote", {"vote": "up"}, 200)
    after = {w["key"]: w["weight"] for w in call("GET", "/api/weights") if w["kind"] == "source"}
    assert after.get("wire-a", 1.0) > before.get("wire-a", 1.0), (before, after)  # votes move weights
    call("POST", f"/api/items/{good['id']}/vote", {"vote": "down"}, 200)
    assert all(i["id"] != good["id"] for i in call("GET", "/api/items?status=presented"))
    print("flywheel ok")

    env = {**os.environ, "LOOP_URL": BASE}
    out = subprocess.run([sys.executable, os.path.join(os.path.dirname(os.path.abspath(__file__)), "client.py"),
                          "--json", "status"], env=env, capture_output=True, text=True, timeout=15)
    assert out.returncode == 0 and "agents_working_now" in json.loads(out.stdout), out.stderr
    out = subprocess.run([sys.executable, os.path.join(os.path.dirname(os.path.abspath(__file__)), "client.py"),
                          "--json", "feed"], env=env, capture_output=True, text=True, timeout=15)
    assert out.returncode == 0 and isinstance(json.loads(out.stdout), list), out.stderr
    print("cli ok")
    print("ALL CHECKS PASSED")

if __name__ == "__main__":
    test_openrouter_mocked()
    test_jev_wire_and_resume()
    env = {k: v for k, v in os.environ.items() if k != "OPENROUTER_API_KEY"}  # gate never spends
    env.update({"PORT": "18099", "LOOP_DB": tempfile.NamedTemporaryFile(suffix=".db", delete=False).name,
                "JEV_MOCK": "1"})  # labelled mock in the gate; the real wire is unit-tested above
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
