"""Loop app: in-the-loop >> near-the-loop >> out-of-the-loop task runner.

Stdlib only. Run: python3 server.py  (PORT env, default 8000, DB env LOOP_DB)
"""
import json, os, sqlite3, threading, time, urllib.request, urllib.error
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

HERE = Path(__file__).parent
DB = os.environ.get("LOOP_DB", str(HERE / "loop.db"))
CONF_THRESHOLD = 0.7
RUN_TIMEOUT_S = 60          # run + decision-wait guardrail
MAX_BODY = 1_000_000
LOOP_TOKEN = os.environ.get("LOOP_TOKEN", "")  # if set, /api needs X-Loop-Token; unset = localhost dev only
OPENROUTER_API_KEY = os.environ.get("OPENROUTER_API_KEY", "")
OPENROUTER_MODEL = os.environ.get("OPENROUTER_MODEL", "openai/gpt-4o-mini")
OPENROUTER_ESCALATION_MODEL = os.environ.get("OPENROUTER_ESCALATION_MODEL", "openai/gpt-4o")
OPENROUTER_MAX_TOKENS = int(os.environ.get("OPENROUTER_MAX_TOKENS", "500"))
PROMOTE_AFTER = {"in": 3, "near": 5}   # consecutive successes to earn promotion
MODES = ["in", "near", "out"]

# Executor templates: (name, confidence, risky).
TEMPLATES = {
    "summarize": [("draft summary", 0.95, False), ("extract key points", 0.90, False)],
    "classify": [("triage ticket", 0.55, False), ("assign team", 0.90, True)],
    "report": [("outline", 0.95, False), ("write draft", 0.92, False), ("publish", 0.88, True)],
}

# shortcut: one connection + lock serialises all runs, upgrade to per-thread connections when parallel out runs matter
db_lock = threading.Lock()
db = sqlite3.connect(DB, check_same_thread=False)
db.row_factory = sqlite3.Row

def init_db():
    with db_lock, db:
        db.executescript("""
        CREATE TABLE IF NOT EXISTS task_types(
          id INTEGER PRIMARY KEY, name TEXT UNIQUE, template TEXT, mode TEXT DEFAULT 'in',
          consecutive_success INTEGER DEFAULT 0, failures INTEGER DEFAULT 0);
        CREATE TABLE IF NOT EXISTS runs(
          id INTEGER PRIMARY KEY, task_type_id INTEGER, input TEXT, mode TEXT,
          status TEXT, created_at REAL, finished_at REAL);
        CREATE TABLE IF NOT EXISTS steps(
          id INTEGER PRIMARY KEY, run_id INTEGER, idx INTEGER, name TEXT,
          status TEXT, output TEXT, confidence REAL, risky INTEGER, pause_reason TEXT);
        CREATE TABLE IF NOT EXISTS decisions(
          id INTEGER PRIMARY KEY, run_id INTEGER, step_id INTEGER, action TEXT,
          edited_output TEXT, created_at REAL);
        CREATE TABLE IF NOT EXISTS audit(
          id INTEGER PRIMARY KEY, run_id INTEGER, event TEXT, detail TEXT, created_at REAL);
        """)
        for name in TEMPLATES:
            db.execute("INSERT OR IGNORE INTO task_types(name,template) VALUES(?,?)", (name, name))
        # No runner threads survive a restart; stale live runs can never finish.
        db.execute("UPDATE runs SET status='killed',finished_at=strftime('%s','now') "
                   "WHERE status IN ('queued','running','awaiting_decision','paused')")
        db.execute("UPDATE steps SET status='rejected' WHERE status IN ('pending','awaiting_decision')")

def q(sql, args=(), one=False):
    with db_lock:
        rows = db.execute(sql, args).fetchall()
    return (dict(rows[0]) if rows else None) if one else [dict(r) for r in rows]

def x(sql, args=()):
    with db_lock, db:
        cur = db.execute(sql, args)
    return cur.lastrowid

def audit(run_id, event, detail=""):
    x("INSERT INTO audit(run_id,event,detail,created_at) VALUES(?,?,?,?)",
      (run_id, event, detail, time.time()))

# In-memory controls for live run threads (not persisted).
CTRL = {}  # run_id -> {"kill":bool,"paused":bool,"decision":dict|None,"wake":Event}

def ctrl(run_id):
    return CTRL.setdefault(run_id, {"kill": False, "paused": False,
                                    "decision": None, "wake": threading.Event()})

def run_step(step_name, text, confidence):
    """Real executor via OpenRouter when OPENROUTER_API_KEY is set, else stub (no spend).

    Input containing FAIL simulates a step failure (kept for failure-path tests).
    Escalates to OPENROUTER_ESCALATION_MODEL when template confidence < threshold.
    Returns (output, confidence); raises RuntimeError on any model/parse failure
    so the loop's pause/alert handling engages.
    """
    if "FAIL" in text:
        raise RuntimeError("simulated step failure (FAIL in input)")
    if not OPENROUTER_API_KEY:
        return f"{step_name}: done for '{text[:40]}'", confidence
    model = OPENROUTER_ESCALATION_MODEL if confidence < CONF_THRESHOLD else OPENROUTER_MODEL
    prompt = (
        f"Do this step: {step_name}\nInput:\n{text[:4000]}\n\n"
        'Reply with ONLY JSON: {"output": "<result>", "confidence": <0.0-1.0>}'
    )
    req = urllib.request.Request(
        "https://openrouter.ai/api/v1/chat/completions",
        data=json.dumps({"model": model, "temperature": 0.2,
                         "max_tokens": OPENROUTER_MAX_TOKENS,
                         "messages": [{"role": "user", "content": prompt}]}).encode(),
        headers={"Authorization": f"Bearer {OPENROUTER_API_KEY}",
                 "Content-Type": "application/json", "X-Title": "loop-app"})
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            content = json.loads(r.read())["choices"][0]["message"]["content"]
    except Exception as e:
        raise RuntimeError(f"OpenRouter call failed: {e}") from e
    try:
        parsed = json.loads(content.strip().removeprefix("```json").removeprefix("```").removesuffix("```").strip())
        out, conf = parsed["output"], float(parsed["confidence"])
        if not isinstance(out, str) or not out:
            raise ValueError("empty output")
    except Exception as e:
        raise RuntimeError(f"OpenRouter returned unparseable output: {content[:120]!r}") from e
    return out, min(1.0, max(0.0, conf))

def wait_decision(run_id, step_id, reason):
    """Block until decide/control acts. Returns decision dict or {'action':'kill'}."""
    c = ctrl(run_id)
    c["decision"] = None
    c["wake"].clear()
    x("UPDATE steps SET status='awaiting_decision',pause_reason=? WHERE id=?", (reason, step_id))
    x("UPDATE runs SET status='awaiting_decision' WHERE id=?", (run_id,))
    audit(run_id, "paused", f"step {step_id}: {reason}")
    deadline = time.time() + RUN_TIMEOUT_S
    while True:
        c["wake"].wait(0.1)
        if c["kill"] or time.time() > deadline:
            return {"action": "kill"}
        if c["decision"]:
            return c["decision"]

def wait_if_paused(run_id):
    c = ctrl(run_id)
    if c["paused"] and not c["kill"]:
        x("UPDATE runs SET status='paused' WHERE id=?", (run_id,))
        while c["paused"] and not c["kill"]:
            time.sleep(0.05)
    if not c["kill"]:
        x("UPDATE runs SET status='running' WHERE id=?", (run_id,))

def finish(run_id, tt_id, status):
    x("UPDATE runs SET status=?,finished_at=? WHERE id=?", (status, time.time(), run_id))
    audit(run_id, f"run_{status}")
    if status == "completed":
        x("UPDATE task_types SET consecutive_success=consecutive_success+1 WHERE id=?", (tt_id,))
    else:
        x("UPDATE task_types SET consecutive_success=0, failures=failures+1 WHERE id=?", (tt_id,))
        tt = q("SELECT mode FROM task_types WHERE id=?", (tt_id,), one=True)
        if tt and tt["mode"] == "out":  # out-of-loop failure demotes automatically
            x("UPDATE task_types SET mode='near' WHERE id=?", (tt_id,))
            audit(run_id, "demoted", "out -> near after failure")

def runner(run_id):
    run = q("SELECT * FROM runs WHERE id=?", (run_id,), one=True)
    tt = q("SELECT * FROM task_types WHERE id=?", (run["task_type_id"],), one=True)
    mode, c, started = run["mode"], ctrl(run_id), time.time()
    x("UPDATE runs SET status='running' WHERE id=?", (run_id,))
    audit(run_id, "run_started", f"mode={mode}")
    try:
        for idx, (name, conf, risky) in enumerate(TEMPLATES[tt["template"]]):
            if c["kill"] or time.time() - started > RUN_TIMEOUT_S:
                return finish(run_id, tt["id"], "killed")
            wait_if_paused(run_id)
            if c["kill"]:
                return finish(run_id, tt["id"], "killed")
            step_id = x("INSERT INTO steps(run_id,idx,name,status,confidence,risky) VALUES(?,?,?,'pending',?,?)",
                        (run_id, idx, name, conf, int(risky)))
            reason = ("approval required (in-the-loop)" if mode == "in" else
                      "low confidence" if mode == "near" and conf < CONF_THRESHOLD else
                      "risky step" if mode == "near" and risky else None)
            if reason:
                d = wait_decision(run_id, step_id, reason)
                if d["action"] in ("kill", "reject"):
                    x("UPDATE steps SET status='rejected' WHERE id=?", (step_id,))
                    return finish(run_id, tt["id"], "killed")
                x("UPDATE runs SET status='running' WHERE id=?", (run_id,))
                if d.get("edited_output"):
                    x("UPDATE steps SET status='done',output=? WHERE id=?", (d["edited_output"], step_id))
                    audit(run_id, "step_edited_done", f"step {step_id}")
                    continue
            while True:
                try:
                    out, got_conf = run_step(name, run["input"], conf)
                    x("UPDATE steps SET status='done',output=?,confidence=? WHERE id=?", (out, got_conf, step_id))
                    audit(run_id, "step_done", f"step {step_id}: {name}")
                    break
                except RuntimeError as e:
                    audit(run_id, "step_failed", f"step {step_id}: {e}")
                    if mode == "out":
                        x("UPDATE steps SET status='failed' WHERE id=?", (step_id,))
                        audit(run_id, "alert", f"out-of-loop failure: {e}")
                        return finish(run_id, tt["id"], "failed")
                    d = wait_decision(run_id, step_id, f"failure: {e} (approve=skip step, reject=kill run)")
                    if d["action"] in ("kill", "reject"):
                        x("UPDATE steps SET status='failed' WHERE id=?", (step_id,))
                        return finish(run_id, tt["id"], "failed")
                    x("UPDATE steps SET status='skipped',output='skipped after failure by human' WHERE id=?", (step_id,))
                    x("UPDATE runs SET status='running' WHERE id=?", (run_id,))
                    break
        return finish(run_id, tt["id"], "completed")
    finally:
        CTRL.pop(run_id, None)

def eligible(tt):
    need = PROMOTE_AFTER.get(tt["mode"])
    return bool(need and tt["consecutive_success"] >= need)

def tt_view(tt):
    return {**tt, "promotion_eligible": eligible(tt),
            "next_mode": MODES[MODES.index(tt["mode"]) + 1] if tt["mode"] != "out" else None}

class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass

    def send_json(self, code, obj):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def authorized(self):
        if not LOOP_TOKEN:
            return True
        return self.headers.get("X-Loop-Token") == LOOP_TOKEN or \
            self.headers.get("Authorization") == f"Bearer {LOOP_TOKEN}"

    def body(self):
        n = int(self.headers.get("Content-Length") or 0)
        if n < 0 or n > MAX_BODY:
            raise ValueError("body too large")
        return json.loads(self.rfile.read(n) or b"{}")

    def do_GET(self):
        parts = [p for p in self.path.split("?")[0].split("/") if p]
        if parts[:1] == ["api"] and not self.authorized():
            return self.send_json(401, {"error": "unauthorized"})
        if not parts:
            page = (HERE / "static" / "index.html").read_bytes()
            self.send_response(200)
            self.send_header("Content-Type", "text/html")
            self.send_header("Content-Length", str(len(page)))
            self.end_headers()
            return self.wfile.write(page)
        if parts == ["api", "task-types"]:
            return self.send_json(200, [tt_view(t) for t in q("SELECT * FROM task_types ORDER BY id")])
        if parts == ["api", "runs"]:
            return self.send_json(200, q("SELECT * FROM runs ORDER BY id DESC LIMIT 50"))
        if len(parts) == 3 and parts[:2] == ["api", "runs"]:
            run = q("SELECT * FROM runs WHERE id=?", (parts[2],), one=True)
            if not run: return self.send_json(404, {"error": "run not found"})
            run["steps"] = q("SELECT * FROM steps WHERE run_id=? ORDER BY idx", (run["id"],))
            run["audit"] = q("SELECT * FROM audit WHERE run_id=? ORDER BY id", (run["id"],))
            return self.send_json(200, run)
        if parts == ["api", "audit"]:
            return self.send_json(200, q("SELECT * FROM audit ORDER BY id DESC LIMIT 100"))
        return self.send_json(404, {"error": "not found"})

    def do_DELETE(self):
        parts = [p for p in self.path.split("?")[0].split("/") if p]
        if parts[:1] == ["api"] and not self.authorized():
            return self.send_json(401, {"error": "unauthorized"})
        if len(parts) == 3 and parts[:2] == ["api", "runs"]:
            run = q("SELECT * FROM runs WHERE id=?", (parts[2],), one=True)
            if not run: return self.send_json(404, {"error": "run not found"})
            if run["status"] not in ("completed", "killed", "failed"):
                return self.send_json(400, {"error": "only finished runs can be deleted"})
            for table in ("steps", "decisions", "audit"):
                x(f"DELETE FROM {table} WHERE run_id=?", (run["id"],))
            x("DELETE FROM runs WHERE id=?", (run["id"],))
            CTRL.pop(run["id"], None)
            audit(None, "run_deleted", f"run {run['id']} task_type={run['task_type_id']} status={run['status']}")
            return self.send_json(200, {"ok": True, "deleted": run["id"]})
        return self.send_json(404, {"error": "not found"})

    def do_POST(self):
        parts = [p for p in self.path.split("?")[0].split("/") if p]
        if parts[:1] == ["api"] and not self.authorized():
            return self.send_json(401, {"error": "unauthorized"})
        try:
            data = self.body()
        except ValueError:
            return self.send_json(400, {"error": "invalid JSON or Content-Length"})
        if not isinstance(data, dict):
            return self.send_json(400, {"error": "JSON object required"})
        if parts == ["api", "task-types"]:
            name, template = data.get("name", ""), data.get("template", "summarize")
            if not isinstance(name, str) or not name.strip() or template not in TEMPLATES or data.get("mode", "in") not in MODES:
                return self.send_json(400, {"error": "name required, template must be summarize|classify|report, mode must be in|near|out"})
            name = name.strip()
            try:
                tid = x("INSERT INTO task_types(name,template,mode) VALUES(?,?,?)",
                        (name, template, data.get("mode", "in")))
            except sqlite3.IntegrityError:
                return self.send_json(400, {"error": "name already exists"})
            return self.send_json(201, tt_view(q("SELECT * FROM task_types WHERE id=?", (tid,), one=True)))
        if len(parts) == 4 and parts[:2] == ["api", "task-types"] and parts[3] in ("mode", "promote"):
            tt = q("SELECT * FROM task_types WHERE id=?", (parts[2],), one=True)
            if not tt: return self.send_json(404, {"error": "task type not found"})
            if parts[3] == "mode":
                if data.get("mode") not in MODES: return self.send_json(400, {"error": "mode must be in|near|out"})
                x("UPDATE task_types SET mode=? WHERE id=?", (data["mode"], tt["id"]))
            else:
                if tt["mode"] == "out": return self.send_json(400, {"error": "already out-of-the-loop"})
                if not (data.get("force") or eligible(tt)):
                    return self.send_json(400, {"error": "not eligible yet (or send force:true)"})
                nxt = MODES[MODES.index(tt["mode"]) + 1]
                x("UPDATE task_types SET mode=?,consecutive_success=0 WHERE id=?", (nxt, tt["id"]))
                audit(None, "promoted", f"{tt['name']}: {tt['mode']} -> {nxt}")
            return self.send_json(200, tt_view(q("SELECT * FROM task_types WHERE id=?", (tt["id"],), one=True)))
        if parts == ["api", "runs"]:
            tt = q("SELECT * FROM task_types WHERE id=?", (data.get("task_type_id"),), one=True)
            if not tt: return self.send_json(400, {"error": "task_type_id not found"})
            if not isinstance(data.get("input", ""), str):
                return self.send_json(400, {"error": "input must be a string"})
            rid = x("INSERT INTO runs(task_type_id,input,mode,status,created_at) VALUES(?,?,?,'queued',?)",
                    (tt["id"], data.get("input", ""), tt["mode"], time.time()))
            audit(rid, "run_created", f"task={tt['name']} mode={tt['mode']}")
            threading.Thread(target=runner, args=(rid,), daemon=True).start()
            return self.send_json(201, q("SELECT * FROM runs WHERE id=?", (rid,), one=True))
        if len(parts) == 4 and parts[:2] == ["api", "runs"] and parts[3] in ("decide", "control"):
            run = q("SELECT * FROM runs WHERE id=?", (parts[2],), one=True)
            if not run: return self.send_json(404, {"error": "run not found"})
            c = ctrl(run["id"])
            if parts[3] == "control":
                act = data.get("action")
                if act == "pause": c["paused"] = True
                elif act == "resume": c["paused"] = False
                elif act == "kill": c["kill"] = True; c["wake"].set()
                else: return self.send_json(400, {"error": "action must be pause|resume|kill"})
                audit(run["id"], f"control_{act}")
                return self.send_json(200, {"ok": True, "action": act})
            act = data.get("action")
            if act not in ("approve", "reject"):
                return self.send_json(400, {"error": "action must be approve|reject"})
            step = q("SELECT * FROM steps WHERE run_id=? AND status='awaiting_decision' ORDER BY idx", (run["id"],), one=True)
            if not step: return self.send_json(400, {"error": "no step awaiting decision"})
            x("INSERT INTO decisions(run_id,step_id,action,edited_output,created_at) VALUES(?,?,?,?,?)",
              (run["id"], step["id"], act, data.get("edited_output"), time.time()))
            audit(run["id"], f"decision_{act}", f"step {step['id']}")
            c["decision"] = {"action": act, "edited_output": data.get("edited_output")}
            c["wake"].set()
            return self.send_json(200, {"ok": True, "action": act, "step_id": step["id"]})
        return self.send_json(404, {"error": "not found"})

if __name__ == "__main__":
    init_db()
    port = int(os.environ.get("PORT", "8000"))
    host = os.environ.get("HOST", "localhost")  # Dockerfile sets 0.0.0.0 so -p can reach it
    print(f"loop app on http://localhost:{port}", flush=True)
    ThreadingHTTPServer((host, port), H).serve_forever()


