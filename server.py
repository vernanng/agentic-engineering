"""Loop app: in-the-loop >> near-the-loop >> out-of-the-loop task runner.

Stdlib only. Run: python3 server.py  (PORT env, default 8000, DB env LOOP_DB)
"""
import hashlib, json, os, re, sqlite3, sys, threading, time, urllib.request, urllib.error
import xml.etree.ElementTree as ET
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
    # Incident: Detect -> Triage -> Patch -> Ask (risky apply in near) -> Verify.
    "incident": [("detect", 0.95, False), ("triage", 0.90, False), ("prepare fix", 0.85, False),
                 ("apply fix", 0.80, True), ("verify", 0.90, False)],
    # News sweep phases are deterministic (do_sweep), not per-step model calls.
    "news": [("gather", 0.95, False), ("gate", 0.95, False), ("dedupe", 0.95, False),
             ("rank", 0.95, False), ("present", 0.95, False)],
}

# Tiny Software Factory harness: an agent is a composition of capability
# slices (docs/tsf-clone.md #16). Disabling "coding" disables run_step().
SLICES = {
    "factory": "task types, runs, promotion",
    "coding": "run_step execution (read/write/edit/bash equivalent)",
    "pages": "agent-written pages in the center pane",
    "factory-controls": "crons, config, self-modification proposals",
}
THINKING_LEVELS = ["low", "medium", "high", "max"]

DEEPSEEK_FLASH_MODEL = os.environ.get("DEEPSEEK_FLASH_MODEL", "")  # cheap tier when set
JEV_MODEL = os.environ.get("JEV_MODEL", "~typesafe/jev-latest")
JEV_MOCK = os.environ.get("JEV_MOCK", "") == "1"  # explicit dev mock, always labelled mock
JEV_PRICE_PER_M = 0.042  # OpenRouter catalog price for jev input tokens, output free

def default_config():
    return {"model": OPENROUTER_MODEL, "escalation_model": OPENROUTER_ESCALATION_MODEL,
            "max_tokens": OPENROUTER_MAX_TOKENS, "conf_threshold": CONF_THRESHOLD,
            "system_prompt": "", "thinking_level": "medium",
            "enabled_slices": list(SLICES), "context_limit": 4000,
            "jev_model": JEV_MODEL, "news_sources": [],
            "cheap_model": DEEPSEEK_FLASH_MODEL or OPENROUTER_MODEL}

def validate_config_patch(patch):
    """Return a cleaned partial config, or raise ValueError."""
    if not isinstance(patch, dict) or not patch:
        raise ValueError("config object required")
    out = {}
    for k, v in patch.items():
        if k in ("model", "escalation_model", "system_prompt"):
            if not isinstance(v, str): raise ValueError(f"{k} must be a string")
        elif k == "max_tokens":
            if not isinstance(v, int) or isinstance(v, bool) or not 1 <= v <= 8000: raise ValueError("max_tokens must be 1..8000")
        elif k == "conf_threshold":
            if not isinstance(v, (int, float)) or isinstance(v, bool) or not 0 <= v <= 1: raise ValueError("conf_threshold must be 0..1")
            v = float(v)
        elif k == "thinking_level":
            if v not in THINKING_LEVELS: raise ValueError("thinking_level must be low|medium|high|max")
        elif k == "enabled_slices":
            if not isinstance(v, list) or not v or not all(s in SLICES for s in v): raise ValueError("enabled_slices must be a non-empty subset of factory|coding|pages|factory-controls")
        elif k == "context_limit":
            if not isinstance(v, int) or isinstance(v, bool) or not 100 <= v <= 100000: raise ValueError("context_limit must be 100..100000")
        elif k == "jev_model":
            if not isinstance(v, str) or not v: raise ValueError("jev_model must be a non-empty string")
        elif k == "cheap_model":
            if not isinstance(v, str) or not v: raise ValueError("cheap_model must be a non-empty string")
        elif k == "news_sources":
            if not isinstance(v, list) or not all(isinstance(u, str) for u in v): raise ValueError("news_sources must be a list of URL strings")
        else:
            raise ValueError(f"unknown config key: {k}")
        out[k] = v
    return out

# Agent access parity (docs/tsf-clone.md #9): every UI control has a tool.
TOOLS = [
    {"name": "list_task_types", "method": "GET", "path": "/api/task-types", "ui": "task list"},
    {"name": "create_task_type", "method": "POST", "path": "/api/task-types", "ui": "mode/promote controls"},
    {"name": "set_mode", "method": "POST", "path": "/api/task-types/:id/mode", "ui": "mode segmented control"},
    {"name": "promote", "method": "POST", "path": "/api/task-types/:id/promote", "ui": "Promote button"},
    {"name": "list_runs", "method": "GET", "path": "/api/runs", "ui": "run list"},
    {"name": "start_run", "method": "POST", "path": "/api/runs", "ui": "Run button"},
    {"name": "get_run", "method": "GET", "path": "/api/runs/:id", "ui": "selected run view"},
    {"name": "decide", "method": "POST", "path": "/api/runs/:id/decide", "ui": "Approve / Reject buttons"},
    {"name": "control", "method": "POST", "path": "/api/runs/:id/control", "ui": "pause/resume/kill buttons"},
    {"name": "delete_run", "method": "DELETE", "path": "/api/runs/:id", "ui": "delete button"},
    {"name": "list_audit", "method": "GET", "path": "/api/audit", "ui": "audit trail"},
    {"name": "list_crons", "method": "GET", "path": "/api/crons", "ui": "cron list"},
    {"name": "create_cron", "method": "POST", "path": "/api/crons", "ui": "cron form"},
    {"name": "delete_cron", "method": "DELETE", "path": "/api/crons/:id", "ui": "cron delete button"},
    {"name": "list_pages", "method": "GET", "path": "/api/pages", "ui": "page list"},
    {"name": "create_page", "method": "POST", "path": "/api/pages", "ui": "page form"},
    {"name": "edit_page", "method": "POST", "path": "/api/pages/:id", "ui": "page form"},
    {"name": "delete_page", "method": "DELETE", "path": "/api/pages/:id", "ui": "page delete button"},
    {"name": "get_config", "method": "GET", "path": "/api/config", "ui": "agent panel config"},
    {"name": "update_config", "method": "PUT", "path": "/api/config", "ui": "agent panel config (human only)"},
    {"name": "propose_config", "method": "POST", "path": "/api/proposals", "ui": "agent self-modification (needs human decide)"},
    {"name": "list_proposals", "method": "GET", "path": "/api/proposals", "ui": "proposal list"},
    {"name": "decide_proposal", "method": "POST", "path": "/api/proposals/:id/decide", "ui": "proposal Approve / Reject"},
    {"name": "create_factory", "method": "POST", "path": "/api/factories", "ui": "factory init form / --init-factory CLI"},
    {"name": "list_tools", "method": "GET", "path": "/api/tools", "ui": "agent panel tools"},
    {"name": "trigger_webhook", "method": "POST", "path": "/api/triggers/:task_type_id", "ui": "webhook (no UI control — external trigger)"},
    {"name": "ingest_event", "method": "POST", "path": "/api/events", "ui": "log-event stream (external trigger)"},
    {"name": "ingest_item", "method": "POST", "path": "/api/items", "ui": "news gather (sweep also fetches sources)"},
    {"name": "list_items", "method": "GET", "path": "/api/items", "ui": "news feed"},
    {"name": "vote_item", "method": "POST", "path": "/api/items/:id/vote", "ui": "card up/down vote"},
    {"name": "list_weights", "method": "GET", "path": "/api/weights", "ui": "taste weights"},
    {"name": "set_weight", "method": "POST", "path": "/api/weights", "ui": "taste weight editor"},
    {"name": "executions", "method": "GET", "path": "/api/executions", "ui": "agent panel executions"},
]

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
        CREATE TABLE IF NOT EXISTS crons(
          id INTEGER PRIMARY KEY, name TEXT, schedule_s INTEGER, prompt TEXT,
          task_type_id INTEGER, enabled INTEGER DEFAULT 1, last_run_at REAL, next_run_at REAL);
        CREATE TABLE IF NOT EXISTS pages(
          id INTEGER PRIMARY KEY, title TEXT, body TEXT, created_by TEXT,
          created_at REAL, updated_at REAL);
        CREATE TABLE IF NOT EXISTS settings(id INTEGER PRIMARY KEY, config TEXT);
        CREATE TABLE IF NOT EXISTS proposals(
          id INTEGER PRIMARY KEY, task_type_id INTEGER, config TEXT, status TEXT,
          created_at REAL);
        CREATE TABLE IF NOT EXISTS items(
          id INTEGER PRIMARY KEY, title TEXT, url TEXT, body TEXT, source TEXT, hash TEXT UNIQUE,
          hype REAL, facts REAL, signal REAL, importance REAL, status TEXT, created_at REAL);
        CREATE TABLE IF NOT EXISTS weights(kind TEXT, key TEXT, weight REAL, PRIMARY KEY(kind, key));
        CREATE TABLE IF NOT EXISTS jev_calls(
          id INTEGER PRIMARY KEY, run_id INTEGER, command TEXT, model TEXT,
          input_tokens INTEGER, cost REAL, mock INTEGER, created_at REAL);
        """)
        for col, ddl in (("tokens", "ALTER TABLE steps ADD COLUMN tokens INTEGER DEFAULT 0"),
                         ("cost", "ALTER TABLE steps ADD COLUMN cost REAL DEFAULT 0")):
            try: db.execute(ddl)
            except Exception: pass  # column already present
        for topic in ("agent", "model", "release"):
            db.execute("INSERT OR IGNORE INTO weights(kind,key,weight) VALUES('topic',?,1.0)", (topic,))
        db.execute("INSERT OR IGNORE INTO settings(id,config) VALUES(1,?)",
                   (json.dumps(default_config()),))
        for name in TEMPLATES:
            db.execute("INSERT OR IGNORE INTO task_types(name,template) VALUES(?,?)", (name, name))
        # Durability: queued runs resume on boot (resume_queued); runs that
        # were mid-flight can never finish and are killed, done steps stay done.
        db.execute("UPDATE runs SET status='killed',finished_at=strftime('%s','now') "
                   "WHERE status IN ('running','awaiting_decision','paused')")
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

def get_config():
    """Core-four config (docs/tsf-clone.md #10); env defaults until first save."""
    cfg = default_config()
    try:
        row = q("SELECT config FROM settings WHERE id=1", one=True)
        if row:
            cfg.update(json.loads(row["config"]))
    except Exception:
        pass  # settings table absent (unit-import without init_db) -> defaults
    return cfg

def save_config(patch):
    cfg = {**get_config(), **validate_config_patch(patch)}
    x("INSERT OR REPLACE INTO settings(id,config) VALUES(1,?)", (json.dumps(cfg),))
    return cfg

def start_run(tt, text):
    rid = x("INSERT INTO runs(task_type_id,input,mode,status,created_at) VALUES(?,?,?,'queued',?)",
            (tt["id"], text, tt["mode"], time.time()))
    audit(rid, "run_created", f"task={tt['name']} mode={tt['mode']}")
    threading.Thread(target=sweep_runner if tt["template"] == "news" else runner, args=(rid,), daemon=True).start()
    return q("SELECT * FROM runs WHERE id=?", (rid,), one=True)

# shortcut: cron lives in this process and dies with it; next_run_at is
# durable in SQLite and recomputed on boot, so a restart delays, never loses.
def cron_loop():
    while True:
        time.sleep(1)
        try:
            now = time.time()
            for cron in q("SELECT * FROM crons WHERE enabled=1 AND next_run_at IS NOT NULL AND next_run_at<=?", (now,)):
                x("UPDATE crons SET last_run_at=?,next_run_at=? WHERE id=?",
                  (now, now + cron["schedule_s"], cron["id"]))  # claim first: never double-fires
                tt = q("SELECT * FROM task_types WHERE id=?", (cron["task_type_id"],), one=True)
                if not tt:
                    continue
                run = fire_trigger("cron", tt, cron["prompt"])
                audit(run["id"], "cron_fired", f"cron {cron['id']}: {cron['name']}")
        except Exception:
            pass  # a bad cron must never kill the scheduler

def create_cron(data):
    name, prompt = data.get("name"), data.get("prompt")
    sched, tt_id = data.get("schedule_s"), data.get("task_type_id")
    if not isinstance(name, str) or not name.strip() or not isinstance(prompt, str) or not prompt:
        raise ValueError("cron name and prompt required")
    if not isinstance(sched, int) or isinstance(sched, bool) or not 1 <= sched <= 86400:
        raise ValueError("schedule_s must be 1..86400 (seconds)")
    tt = q("SELECT * FROM task_types WHERE id=?", (tt_id,), one=True)
    if not tt:
        raise ValueError("task_type_id not found")
    now = time.time()
    cid = x("INSERT INTO crons(name,schedule_s,prompt,task_type_id,enabled,last_run_at,next_run_at) VALUES(?,?,?,?,1,NULL,?)",
            (name.strip(), sched, prompt, tt["id"], now + sched))
    return q("SELECT * FROM crons WHERE id=?", (cid,), one=True)

def create_factory(data):
    """Initialize, configure, specialize (docs/tsf-clone.md #4). Starts in."""
    name, template = data.get("name"), data.get("template", "summarize")
    if not isinstance(name, str) or not name.strip() or template not in TEMPLATES:
        raise ValueError("name required, template must be summarize|classify|report|incident|news")
    try:
        tid = x("INSERT INTO task_types(name,template,mode) VALUES(?,?,?)", (name.strip(), template, "in"))
    except sqlite3.IntegrityError:
        raise ValueError("name already exists")
    tt = q("SELECT * FROM task_types WHERE id=?", (tid,), one=True)
    crons = [create_cron({**c, "task_type_id": tid}) for c in data.get("crons") or []]
    page = None
    if data.get("starter_page"):
        sp = data["starter_page"]
        if not isinstance(sp.get("title"), str) or not sp["title"].strip() or not isinstance(sp.get("body", ""), str):
            raise ValueError("starter_page needs a title and string body")
        pid = x("INSERT INTO pages(title,body,created_by,created_at,updated_at) VALUES(?,?,?,?,?)",
                (sp["title"].strip(), sp.get("body", ""), "factory-init", time.time(), time.time()))
        page = q("SELECT * FROM pages WHERE id=?", (pid,), one=True)
    audit(None, "factory_created", f"{tt['name']} template={template} crons={len(crons)}")
    return {"task_type": tt_view(tt), "crons": crons, "page": page}

def fire_trigger(kind, tt, text):
    """Every trigger (prompt/click/cron/webhook/event) enters via start_run."""
    run = start_run(tt, text)
    audit(run["id"], "trigger_fired", f"kind={kind} task={tt['name']}")
    return run

# --- Real Jev via OpenRouter (docs/leaving-the-loop-clone.md #6) ---
_jev_cache = {}

def _render(obj, vars):
    if isinstance(obj, str):
        m = re.fullmatch(r"\{\{(\w+)\}\}", obj)
        if m: return vars.get(m.group(1), "")
        return re.sub(r"\{\{(\w+)\}\}", lambda mm: str(vars.get(mm.group(1), "")), obj)
    if isinstance(obj, dict): return {k: _render(v, vars) for k, v in obj.items()}
    if isinstance(obj, list): return [_render(v, vars) for v in obj]
    return obj

def load_jev(name):
    if name not in _jev_cache:
        try:
            with open(HERE / "prompt_engineering" / "jev" / name / "command.json") as f:
                _jev_cache[name] = json.load(f)
        except Exception as e:
            raise RuntimeError(f"jev command {name} unreadable: {e}") from e
    return _jev_cache[name]

def _jev_mock_answers(state, questions):
    text = json.dumps(state).lower() if not isinstance(state, dict) else \
        (str(state.get("title", "")) + " " + str(state.get("excerpt", ""))).lower()
    hot = any(w in text for w in ("release", "available", "agent", "model", "flag"))
    answers = {}
    for qid, qu in questions.items():
        if qu["type"] == "noul":
            answers[qid] = {"type": "noul", "noul": 0.9 if hot else 0.15}
        elif qu["type"] == "choice":
            opts = list(qu.get("criteria") or {})
            answers[qid] = {"type": "choice", "choice": opts[0] if opts else "", "probabilities": {}, "confidence": 0.5}
        else:
            top = len(qu.get("criteria") or [1, 2]) - 1
            pos = 0.85 if hot else 0.2
            if qid == "hype": pos = 0.85 if ("changes everything" in text or "revolutionary" in text) else 0.25
            if qid == "facts": pos = 0.85 if re.search(r"\d|version", text) else 0.3
            answers[qid] = {"type": "score", "score": pos * top, "legend": {str(i): c for i, c in enumerate(qu.get("criteria") or [])}, "probabilities": {}, "confidence": 0.5}
    return answers

def jev_classify(state, questions, command="", run_id=None):
    """One Jev Decisions call. Raises RuntimeError on any failure (fail closed)."""
    if JEV_MOCK:
        answers = _jev_mock_answers(state, questions)
        x("INSERT INTO jev_calls(run_id,command,model,input_tokens,cost,mock,created_at) VALUES(?,?,?,0,0,1,?)",
          (run_id, command, "mock", time.time()))
        return answers
    if not OPENROUTER_API_KEY:
        raise RuntimeError("jev unavailable (no OPENROUTER_API_KEY); set JEV_MOCK=1 for explicit dev mock")
    cfg = get_config()
    payload = {"model": cfg["jev_model"], "state": state, "questions": questions}
    req = urllib.request.Request("https://openrouter.ai/api/alpha/decisions",
        data=json.dumps(payload).encode(),
        headers={"Authorization": f"Bearer {OPENROUTER_API_KEY}", "Content-Type": "application/json", "X-Title": "loop-app"})
    last = None
    for attempt in range(3):
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                data = json.loads(r.read())
            answers = data["answers"]
            if not isinstance(answers, dict) or not all(isinstance(a, dict) and "type" in a for a in answers.values()):
                raise ValueError("malformed jev answers")
            usage = data.get("usage") or {}
            tokens = int(usage.get("input_tokens") or 0)
            cost = usage.get("cost") if isinstance(usage.get("cost"), (int, float)) else tokens / 1_000_000 * JEV_PRICE_PER_M
            x("INSERT INTO jev_calls(run_id,command,model,input_tokens,cost,mock,created_at) VALUES(?,?,?,?,?,0,?)",
              (run_id, command, data.get("model", cfg["jev_model"]), tokens, float(cost), time.time()))
            return answers
        except urllib.error.HTTPError as e:
            last = e
            if e.code not in (429, 502, 503, 529): break
            time.sleep(0.5 * (attempt + 1))
        except Exception as e:
            raise RuntimeError(f"jev call failed: {e}") from e
    raise RuntimeError(f"jev call failed: {last}")

def run_jev(name, vars, run_id=None):
    cmd = load_jev(name)
    questions = _render(cmd["questions"], vars)
    return jev_classify(_render(cmd["state"], vars), questions, command=name, run_id=run_id), cmd.get("policy") or {}

def score100(answer):
    top = len(answer.get("legend") or answer.get("probabilities") or [1, 2]) - 1
    return round(min(1.0, max(0.0, answer["score"] / top)) * 100) if top > 0 else 0

def topic_keywords():
    return [{"key": w["key"], "weight": w["weight"]} for w in q("SELECT * FROM weights WHERE kind='topic' ORDER BY key")]

def taste_examples():
    liked = [i["title"] for i in q("SELECT title FROM items WHERE status='presented' ORDER BY id DESC LIMIT 3")]
    disliked = [i["title"] for i in q("SELECT title FROM items WHERE status='dismissed' ORDER BY id DESC LIMIT 3")]
    return {"liked": liked, "disliked": disliked}

# --- News sweep: gather -> gate -> dedupe -> rank -> present ---
def _item_hash(title, body):
    return hashlib.sha256(f"{title}\n{body}".encode()).hexdigest()[:32]

def insert_item(title, url, body, source):
    h = _item_hash(title, body)
    try:
        iid = x("INSERT INTO items(title,url,body,source,hash,status,created_at) VALUES(?,?,?,?,?,'new',?)",
                (title, url, body, source, h, time.time()))
        return iid, True
    except sqlite3.IntegrityError:
        return q("SELECT id FROM items WHERE hash=?", (h,), one=True)["id"], False

def fetch_source(url):
    req = urllib.request.Request(url, headers={"User-Agent": "loop-app news sweep"})
    with urllib.request.urlopen(req, timeout=10) as r:
        root = ET.fromstring(r.read(5_000_000))  # shortcut: feeds over 5 MB fail parse and are skipped per-source, raise the cap when a real source outgrows it
    out = []
    for node in root.iter():
        if node.tag.split("}")[-1] in ("item", "entry"):
            vals = {c.tag.split("}")[-1]: (c.text or "").strip() for c in node.iter() if c.text}
            if vals.get("title"):
                out.append((vals["title"], vals.get("link", url), vals.get("description", vals.get("summary", ""))))
    return out

def do_sweep(run_id):
    run = q("SELECT * FROM runs WHERE id=?", (run_id,), one=True)
    tt = q("SELECT * FROM task_types WHERE id=?", (run["task_type_id"],), one=True)
    x("UPDATE runs SET status='running' WHERE id=?", (run_id,))
    audit(run_id, "run_started", f"mode={run['mode']} template=news")
    idx = -1
    def phase(name, output):
        nonlocal idx; idx += 1
        x("INSERT INTO steps(run_id,idx,name,status,output,confidence,risky,tokens,cost) VALUES(?,?,?,'done',?,0.95,0,0,0)",
          (run_id, idx, name, output))
        audit(run_id, "step_done", f"{name}: {output}")
    try:
        new_count = 0
        for url in get_config()["news_sources"]:
            try:
                for title, link, desc in fetch_source(url):
                    _, created = insert_item(title, link, desc, url)
                    new_count += int(created)
            except Exception as e:
                audit(run_id, "source_skipped", f"{url}: {e}")  # one bad source never kills a sweep
        phase("gather", f"{new_count} new items")
        gated, folded = [], 0
        presented_hashes = {i["hash"] for i in q("SELECT hash FROM items WHERE status='presented'")}
        for item in q("SELECT * FROM items WHERE status='new' ORDER BY id LIMIT 50"):
            if item["hash"] in presented_hashes:
                x("UPDATE items SET status='dup' WHERE id=?", (item["id"],)); folded += 1; continue
            vars = {"title": item["title"], "source": item["source"], "excerpt": item["body"][:800],
                    "published_at": "", "topic_keywords": topic_keywords(), "taste_examples": taste_examples(),
                    "topic_choices": {w["key"]: None for w in topic_keywords()} or {"agent": None}}
            rel, rel_pol = run_jev("relevance", vars, run_id)
            worth, worth_pol = run_jev("worth-attention", vars, run_id)
            if score100(rel["relevance"]) / 100 < rel_pol.get("relevance_min", 0.45) or worth["worth"]["noul"] < worth_pol.get("worth_min", 0.35):
                x("UPDATE items SET status='gated' WHERE id=?", (item["id"],))
            else:
                gated.append(item)
        phase("gate", f"{len(gated)} passed, {folded} hash-dupes folded")
        kept = []
        for item in gated:  # shortcut: jev same-story only when titles collide; hash+title cover the common case
            dupe = next((k for k in kept if k["title"].lower() == item["title"].lower()), None)
            if dupe:
                ans, pol = run_jev("same-story", {"candidate": item["title"], "existing": dupe["title"]}, run_id)
                if ans["same"]["noul"] >= pol.get("same", 0.85):
                    x("UPDATE items SET status='dup' WHERE id=?", (item["id"],)); folded += 1; continue
            kept.append(item)
        phase("dedupe", f"{len(kept)} unique stories")
        topics = topic_keywords()
        for item in kept:
            hfs, _ = run_jev("hfs", {"title": item["title"], "source": item["source"], "excerpt": item["body"][:800],
                                     "findings": "", "section_keywords": {}, "taste_examples": taste_examples()}, run_id)
            imp, _ = run_jev("importance", {"title": item["title"], "source": item["source"], "source_weight": 1,
                                            "published_at": "", "excerpt": item["body"][:800], "topic": "",
                                            "topic_weight": 1, "topic_keywords": topics, "taste_examples": taste_examples()}, run_id)
            text = (item["title"] + " " + item["body"]).lower()
            tw = max([w["weight"] for w in topics if w["key"] in text] or [1.0])
            sw = (q("SELECT weight FROM weights WHERE kind='source' AND key=?", (item["source"],), one=True) or {"weight": 1.0})["weight"]
            age_days = max(0.0, (time.time() - item["created_at"]) / 86400)
            rank = score100(imp["importance"]) * tw * sw / (1 + age_days)
            x("UPDATE items SET hype=?,facts=?,signal=?,importance=?,status='ranked' WHERE id=?",
              (score100(hfs["hype"]), score100(hfs["facts"]), score100(hfs["signal"]), rank, item["id"]))
            item.update({"hype": score100(hfs["hype"]), "facts": score100(hfs["facts"]), "signal": score100(hfs["signal"]), "importance": rank})
        kept.sort(key=lambda i: -(i["importance"] or 0))
        phase("rank", f"ranked {len(kept)}")
        top = kept[:7]
        for item in top:
            x("UPDATE items SET status='presented' WHERE id=?", (item["id"],))
            if (item["hype"] or 0) >= 50 and (item["signal"] or 0) >= 50 and (item["facts"] or 0) < 50:
                audit(run_id, "suspect_combo", f"item {item['id']}: hype+signal without facts — needs human taste")
        cards = "".join(f"<article><h3>{i['title']}</h3><p>Hype {i['hype']} · Facts {i['facts']} · Signal {i['signal']}</p><p>{i['body'][:300]}</p></article>" for i in top)
        pid = x("INSERT INTO pages(title,body,created_by,created_at,updated_at) VALUES(?,?,?,?,?)",
                (f"News sweep {time.strftime('%H:%M')}", cards or "<p>nothing new</p>", "agent", time.time(), time.time()))
        phase("present", f"{len(top)} cards on page {pid}")
        return finish(run_id, tt["id"], "completed")
    except Exception as e:  # a sweep must fail its run, never strand it running
        audit(run_id, "sweep_failed", str(e))
        if run["mode"] == "out":
            audit(run_id, "alert", f"out-of-loop sweep failure: {e}")
        return finish(run_id, tt["id"], "failed")

def resume_queued():
    """Boot durability: queued runs start; done steps are checkpoints."""
    for run in q("SELECT * FROM runs WHERE status='queued'"):
        tt = q("SELECT * FROM task_types WHERE id=?", (run["task_type_id"],), one=True)
        if tt:
            threading.Thread(target=sweep_runner if tt["template"] == "news" else runner, args=(run["id"],), daemon=True).start()
            audit(run["id"], "run_resumed", "queued run resumed after restart")

def sweep_runner(run_id):
    try: do_sweep(run_id)
    finally: CTRL.pop(run_id, None)

# Last real-model usage from run_step() on this thread: (tokens, cost) or None (stub/proxy).
_step_usage = threading.local()

def take_step_usage():
    u = getattr(_step_usage, "value", None)
    _step_usage.value = None
    return u

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
    _step_usage.value = None
    if "FAIL" in text:
        raise RuntimeError("simulated step failure (FAIL in input)")
    cfg = get_config()  # core-four, read live so config/proposals apply at once
    if "coding" not in cfg["enabled_slices"]:
        raise RuntimeError("coding slice disabled (self-modification in effect)")
    if not OPENROUTER_API_KEY:
        return f"{step_name}: done for '{text[:40]}'", confidence
    # Tokenomics tiers: cheap (DeepSeek Flash) for routine steps, main mid, escalation low.
    model = cfg["escalation_model"] if confidence < cfg["conf_threshold"] else \
        cfg["cheap_model"] if confidence >= 0.9 else cfg["model"]
    prompt = (
        (cfg["system_prompt"] + "\n" if cfg["system_prompt"] else "")
        + f"Do this step: {step_name}\nInput:\n{text[:cfg['context_limit']]}\n\n"
        'Reply with ONLY JSON: {"output": "<result>", "confidence": <0.0-1.0>}'
    )
    req = urllib.request.Request(
        "https://openrouter.ai/api/v1/chat/completions",
        data=json.dumps({"model": model, "temperature": 0.2,
                         "max_tokens": cfg["max_tokens"],
                         "messages": [{"role": "user", "content": prompt}]}).encode(),
        headers={"Authorization": f"Bearer {OPENROUTER_API_KEY}",
                 "Content-Type": "application/json", "X-Title": "loop-app"})
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            body = json.loads(r.read())
            content = body["choices"][0]["message"]["content"]
    except Exception as e:
        raise RuntimeError(f"OpenRouter call failed: {e}") from e
    usage = body.get("usage") or {}
    try:
        tokens = int(usage.get("total_tokens") or (int(usage.get("prompt_tokens") or 0) + int(usage.get("completion_tokens") or 0)))
        cost = float(usage.get("cost") or 0)
    except (TypeError, ValueError):
        tokens, cost = 0, 0.0
    if tokens:
        _step_usage.value = (tokens, cost)  # runner stores real usage; absent usage keeps the chars/4 proxy
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
                      "low confidence" if mode == "near" and conf < get_config()["conf_threshold"] else
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
                    tokens, cost = take_step_usage() or ((len(run["input"]) + len(out)) // 4, 0)  # real usage wins; chars/4 proxy under the stub
                    x("UPDATE steps SET status='done',output=?,confidence=?,tokens=?,cost=? WHERE id=?",
                      (out, got_conf, tokens, cost, step_id))
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
        if parts == ["api", "tools"]:
            return self.send_json(200, {"tools": TOOLS, "slices": SLICES})
        if parts == ["api", "config"]:
            return self.send_json(200, get_config())
        if parts == ["api", "crons"]:
            return self.send_json(200, q("SELECT * FROM crons ORDER BY id"))
        if parts == ["api", "pages"]:
            return self.send_json(200, q("SELECT * FROM pages ORDER BY id"))
        if len(parts) == 3 and parts[:2] == ["api", "pages"]:
            page = q("SELECT * FROM pages WHERE id=?", (parts[2],), one=True)
            if not page: return self.send_json(404, {"error": "page not found"})
            return self.send_json(200, page)
        if parts == ["api", "proposals"]:
            return self.send_json(200, q("SELECT * FROM proposals ORDER BY id DESC LIMIT 50"))
        if parts == ["api", "items"]:
            status = self.path.split("status=")[-1] if "status=" in self.path else None
            rows = q("SELECT * FROM items WHERE status=? ORDER BY importance DESC, id DESC LIMIT 50", (status,)) if status \
                else q("SELECT * FROM items ORDER BY id DESC LIMIT 50")
            return self.send_json(200, rows)
        if parts == ["api", "weights"]:
            return self.send_json(200, q("SELECT * FROM weights ORDER BY kind, key"))
        if parts == ["api", "executions"]:
            steps = q("SELECT COALESCE(SUM(tokens),0) AS t, COALESCE(SUM(cost),0) AS c FROM steps", one=True)
            jev = q("SELECT COUNT(*) AS n, COALESCE(SUM(cost),0) AS c, COALESCE(SUM(mock),0) AS m FROM jev_calls", one=True)
            live = q("SELECT COUNT(*) AS n FROM runs WHERE status IN ('queued','running','awaiting_decision','paused')", one=True)["n"]
            crons_on = q("SELECT COUNT(*) AS n FROM crons WHERE enabled=1", one=True)["n"]
            return self.send_json(200, {"step_tokens": steps["t"], "step_cost": steps["c"],
                "jev_calls": jev["n"], "jev_cost": jev["c"], "jev_mock_calls": jev["m"],
                "agents_working_now": live + crons_on, "live_runs": live, "enabled_crons": crons_on,
                "runs_by_status": {r["status"]: r["n"] for r in q("SELECT status, COUNT(*) AS n FROM runs GROUP BY status")}})
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
        if len(parts) == 3 and parts[:2] == ["api", "crons"]:
            cron = q("SELECT * FROM crons WHERE id=?", (parts[2],), one=True)
            if not cron: return self.send_json(404, {"error": "cron not found"})
            x("DELETE FROM crons WHERE id=?", (cron["id"],))
            audit(None, "cron_deleted", f"cron {cron['id']}: {cron['name']}")
            return self.send_json(200, {"ok": True, "deleted": cron["id"]})
        if len(parts) == 3 and parts[:2] == ["api", "pages"]:
            page = q("SELECT * FROM pages WHERE id=?", (parts[2],), one=True)
            if not page: return self.send_json(404, {"error": "page not found"})
            x("DELETE FROM pages WHERE id=?", (page["id"],))
            audit(None, "page_deleted", f"page {page['id']}: {page['title']}")
            return self.send_json(200, {"ok": True, "deleted": page["id"]})
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
                return self.send_json(400, {"error": "name required, template must be summarize|classify|report|incident|news, mode must be in|near|out"})
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
            return self.send_json(201, start_run(tt, data.get("input", "")))
        if parts == ["api", "crons"]:
            try:
                return self.send_json(201, create_cron(data))
            except ValueError as e:
                return self.send_json(400, {"error": str(e)})
        if parts == ["api", "pages"]:
            title, body = data.get("title"), data.get("body", "")
            if not isinstance(title, str) or not title.strip() or not isinstance(body, str):
                return self.send_json(400, {"error": "page title required, body must be a string"})
            pid = x("INSERT INTO pages(title,body,created_by,created_at,updated_at) VALUES(?,?,?,?,?)",
                    (title.strip(), body, data.get("created_by", "human"), time.time(), time.time()))
            audit(None, "page_created", f"page {pid}: {title.strip()}")
            return self.send_json(201, q("SELECT * FROM pages WHERE id=?", (pid,), one=True))
        if len(parts) == 3 and parts[:2] == ["api", "pages"]:
            page = q("SELECT * FROM pages WHERE id=?", (parts[2],), one=True)
            if not page: return self.send_json(404, {"error": "page not found"})
            title, body = data.get("title", page["title"]), data.get("body", page["body"])
            if not isinstance(title, str) or not title.strip() or not isinstance(body, str):
                return self.send_json(400, {"error": "page title required, body must be a string"})
            x("UPDATE pages SET title=?,body=?,updated_at=? WHERE id=?", (title.strip(), body, time.time(), page["id"]))
            audit(None, "page_edited", f"page {page['id']}: {title.strip()}")
            return self.send_json(200, q("SELECT * FROM pages WHERE id=?", (page["id"],), one=True))
        if parts == ["api", "factories"]:
            try:
                return self.send_json(201, create_factory(data))
            except ValueError as e:
                return self.send_json(400, {"error": str(e)})
        if parts == ["api", "proposals"]:
            tt = q("SELECT * FROM task_types WHERE id=?", (data.get("task_type_id"),), one=True)
            if not tt: return self.send_json(400, {"error": "task_type_id not found"})
            if tt["mode"] == "out":
                return self.send_json(400, {"error": "out-of-the-loop agents cannot propose self-modification"})
            try:
                patch = validate_config_patch(data.get("config"))
            except ValueError as e:
                return self.send_json(400, {"error": str(e)})
            pid = x("INSERT INTO proposals(task_type_id,config,status,created_at) VALUES(?,?,'pending',?)",
                    (tt["id"], json.dumps(patch), time.time()))
            audit(None, "proposal_created", f"proposal {pid} task={tt['name']}: {sorted(patch)}")
            return self.send_json(201, q("SELECT * FROM proposals WHERE id=?", (pid,), one=True))
        if len(parts) == 4 and parts[:2] == ["api", "proposals"] and parts[3] == "decide":
            prop = q("SELECT * FROM proposals WHERE id=?", (parts[2],), one=True)
            if not prop: return self.send_json(404, {"error": "proposal not found"})
            if prop["status"] != "pending":
                return self.send_json(400, {"error": "proposal already decided"})
            act = data.get("action")
            if act not in ("approve", "reject"):
                return self.send_json(400, {"error": "action must be approve|reject"})
            if act == "approve":  # self-modification applies only here, by a human
                save_config(json.loads(prop["config"]))
                x("UPDATE proposals SET status='applied' WHERE id=?", (prop["id"],))
                audit(None, "self_modified", f"proposal {prop['id']} applied: {prop['config']}")
            else:
                x("UPDATE proposals SET status='rejected' WHERE id=?", (prop["id"],))
                audit(None, "proposal_rejected", f"proposal {prop['id']}")
            return self.send_json(200, q("SELECT * FROM proposals WHERE id=?", (prop["id"],), one=True))
        if len(parts) == 3 and parts[:2] == ["api", "triggers"]:
            tt = q("SELECT * FROM task_types WHERE id=?", (parts[2],), one=True)
            if not tt: return self.send_json(404, {"error": "task type not found"})
            if not isinstance(data.get("input", ""), str):
                return self.send_json(400, {"error": "input must be a string"})
            return self.send_json(201, fire_trigger("webhook", tt, data.get("input", "")))
        if parts == ["api", "events"]:
            status, message = data.get("status"), data.get("message", "")
            if not isinstance(status, int) or isinstance(status, bool) or not isinstance(message, str):
                return self.send_json(400, {"error": "status (int) and message (string) required"})
            tt = q("SELECT * FROM task_types WHERE id=?", (data.get("task_type_id"),), one=True) \
                or q("SELECT * FROM task_types WHERE template='incident' ORDER BY id", one=True)
            if not tt: return self.send_json(400, {"error": "no incident task type; create one first"})
            if 400 <= status < 600:  # fast path: no classifier needed for 4xx/5xx
                return self.send_json(201, {"started": True, "run": fire_trigger("event", tt, f"[{status}] {message}")})
            try:
                rel, pol = run_jev("relevance", {"title": message, "source": data.get("source", "event"),
                    "excerpt": message, "topic_keywords": topic_keywords(), "taste_examples": taste_examples(),
                    "topic_choices": {w["key"]: None for w in topic_keywords()} or {"agent": None}})
                relevant = score100(rel["relevance"]) / 100 >= pol.get("relevance_min", 0.45)
            except RuntimeError as e:
                return self.send_json(503, {"error": str(e)})
            audit(None, "event_received", f"status={status} relevant={relevant}: {message[:80]}")
            if not relevant:
                return self.send_json(200, {"started": False, "reason": "classified not relevant"})
            return self.send_json(201, {"started": True, "run": fire_trigger("event", tt, f"[{status}] {message}")})
        if parts == ["api", "items"]:
            title, body = data.get("title"), data.get("body", "")
            if not isinstance(title, str) or not title.strip() or not isinstance(body, str):
                return self.send_json(400, {"error": "item title required, body must be a string"})
            iid, created = insert_item(title.strip(), data.get("url", ""), body, data.get("source", "manual"))
            return self.send_json(201 if created else 200, q("SELECT * FROM items WHERE id=?", (iid,), one=True))
        if len(parts) == 4 and parts[:2] == ["api", "items"] and parts[3] == "vote":
            item = q("SELECT * FROM items WHERE id=?", (parts[2],), one=True)
            if not item: return self.send_json(404, {"error": "item not found"})
            vote = data.get("vote")
            if vote not in ("up", "down"): return self.send_json(400, {"error": "vote must be up|down"})
            delta = 0.1 if vote == "up" else -0.1  # the flywheel: votes move weights
            if vote == "down":
                x("UPDATE items SET status='dismissed' WHERE id=?", (item["id"],))
            src = q("SELECT weight FROM weights WHERE kind='source' AND key=?", (item["source"],), one=True)
            x("INSERT OR REPLACE INTO weights(kind,key,weight) VALUES('source',?,?)",
              (item["source"], max(0.1, (src["weight"] if src else 1.0) + delta)))
            text = (item["title"] + " " + item["body"]).lower()
            for w in topic_keywords():
                if w["key"] in text:
                    x("INSERT OR REPLACE INTO weights(kind,key,weight) VALUES('topic',?,?)",
                      (w["key"], max(0.1, w["weight"] + delta)))
            audit(None, f"item_{vote}voted", f"item {item['id']}: {item['title'][:60]}")
            return self.send_json(200, q("SELECT * FROM items WHERE id=?", (item["id"],), one=True))
        if parts == ["api", "weights"]:
            kind, key, weight = data.get("kind"), data.get("key"), data.get("weight")
            if kind not in ("topic", "source") or not isinstance(key, str) or not key.strip() \
                    or not isinstance(weight, (int, float)) or isinstance(weight, bool) or not 0.1 <= weight <= 2:
                return self.send_json(400, {"error": "kind topic|source, key, weight 0.1..2 required"})
            x("INSERT OR REPLACE INTO weights(kind,key,weight) VALUES(?,?,?)", (kind, key.strip(), float(weight)))
            return self.send_json(200, {"kind": kind, "key": key.strip(), "weight": float(weight)})
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

    def do_PUT(self):
        parts = [p for p in self.path.split("?")[0].split("/") if p]
        if parts[:1] == ["api"] and not self.authorized():
            return self.send_json(401, {"error": "unauthorized"})
        if parts == ["api", "config"]:  # human direct config; agents use proposals
            try:
                data = self.body()
                if not isinstance(data, dict):
                    raise ValueError("JSON object required")
                return self.send_json(200, save_config(data))
            except ValueError as e:
                return self.send_json(400, {"error": str(e)})
        return self.send_json(404, {"error": "not found"})

if __name__ == "__main__":
    init_db()
    if sys.argv[1:2] == ["--init-factory"]:  # one command: initialize/configure/specialize
        with open(sys.argv[2]) as f:
            print(json.dumps(create_factory(json.load(f)), indent=1))
        sys.exit(0)
    resume_queued()
    threading.Thread(target=cron_loop, daemon=True).start()
    port = int(os.environ.get("PORT", "8000"))
    host = os.environ.get("HOST", "localhost")  # Dockerfile sets 0.0.0.0 so -p can reach it
    print(f"loop app on http://localhost:{port}", flush=True)
    ThreadingHTTPServer((host, port), H).serve_forever()


