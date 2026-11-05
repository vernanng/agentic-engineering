//! Loop app: in-the-loop >> near-the-loop >> out-of-the-loop task runner.
//! Rust port of server.py. Run: cargo run --release (PORT env, default 8000, DB env LOOP_DB)
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CONF_THRESHOLD: f64 = 0.7;
const RUN_TIMEOUT_S: f64 = 60.0;
const MAX_BODY: usize = 1_000_000;
const JEV_PRICE_PER_M: f64 = 0.042;

fn now() -> f64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs_f64() }
fn env_or(k: &str, d: &str) -> String { std::env::var(k).unwrap_or_else(|_| d.into()) }
fn openrouter_base() -> String { env_or("OPENROUTER_API_BASE", "https://openrouter.ai") }
fn api_key() -> String { std::env::var("OPENROUTER_API_KEY").unwrap_or_default() }
fn jev_mock() -> bool { std::env::var("JEV_MOCK").map(|v| v == "1").unwrap_or(false) }

fn templates() -> Vec<(&'static str, Vec<(&'static str, f64, bool)>)> {
    vec![
        ("summarize", vec![("draft summary", 0.95, false), ("extract key points", 0.90, false)]),
        ("classify", vec![("triage ticket", 0.55, false), ("assign team", 0.90, true)]),
        ("report", vec![("outline", 0.95, false), ("write draft", 0.92, false), ("publish", 0.88, true)]),
        ("incident", vec![("detect", 0.95, false), ("triage", 0.90, false), ("prepare fix", 0.85, false), ("apply fix", 0.80, true), ("verify", 0.90, false)]),
        ("news", vec![("gather", 0.95, false), ("gate", 0.95, false), ("dedupe", 0.95, false), ("rank", 0.95, false), ("present", 0.95, false)]),
    ]
}
fn template_names() -> Vec<&'static str> { templates().iter().map(|t| t.0).collect() }
fn template_steps(name: &str) -> Vec<(&'static str, f64, bool)> {
    templates().into_iter().find(|t| t.0 == name).map(|t| t.1).unwrap_or_default()
}
fn slices() -> Vec<(&'static str, &'static str)> {
    vec![("factory", "task types, runs, promotion"), ("coding", "run_step execution (read/write/edit/bash equivalent)"),
         ("pages", "agent-written pages in the center pane"), ("factory-controls", "crons, config, self-modification proposals")]
}
const MODES: [&str; 3] = ["in", "near", "out"];
const THINKING: [&str; 4] = ["low", "medium", "high", "max"];

fn default_config() -> Value {
    json!({"model": env_or("OPENROUTER_MODEL", "openai/gpt-4o-mini"),
        "escalation_model": env_or("OPENROUTER_ESCALATION_MODEL", "openai/gpt-4o"),
        "max_tokens": std::env::var("OPENROUTER_MAX_TOKENS").ok().and_then(|v| v.parse::<i64>().ok()).unwrap_or(500),
        "conf_threshold": CONF_THRESHOLD, "system_prompt": "", "thinking_level": "medium",
        "enabled_slices": slices().iter().map(|s| s.0).collect::<Vec<_>>(), "context_limit": 4000,
        "jev_model": env_or("JEV_MODEL", "~typesafe/jev-latest"), "news_sources": [],
        "cheap_model": ({ let d = std::env::var("DEEPSEEK_FLASH_MODEL").unwrap_or_default(); if d.is_empty() { env_or("OPENROUTER_MODEL", "openai/gpt-4o-mini") } else { d } })})
}

fn validate_config_patch(patch: &Value) -> Result<Value, String> {
    let obj = patch.as_object().filter(|o| !o.is_empty()).ok_or("config object required")?;
    let slice_names: Vec<&str> = slices().iter().map(|s| s.0).collect();
    let mut out = json!({});
    for (k, v) in obj {
        match k.as_str() {
            "model" | "escalation_model" | "system_prompt" => { if !v.is_string() { return Err(format!("{k} must be a string")); } }
            "max_tokens" => { let n = v.as_i64().filter(|n| (1..=8000).contains(n)).ok_or("max_tokens must be 1..8000")?; out[k] = json!(n); continue; }
            "conf_threshold" => { let n = v.as_f64().filter(|n| (0.0..=1.0).contains(n)).ok_or("conf_threshold must be 0..1")?; out[k] = json!(n); continue; }
            "thinking_level" => { if !THINKING.contains(&v.as_str().unwrap_or("")) { return Err("thinking_level must be low|medium|high|max".into()); } }
            "enabled_slices" => { let a = v.as_array().filter(|a| !a.is_empty() && a.iter().all(|s| slice_names.contains(&s.as_str().unwrap_or("")))).ok_or("enabled_slices must be a non-empty subset of factory|coding|pages|factory-controls")?; out[k] = json!(a); continue; }
            "context_limit" => { let n = v.as_i64().filter(|n| (100..=100000).contains(n)).ok_or("context_limit must be 100..100000")?; out[k] = json!(n); continue; }
            "jev_model" | "cheap_model" => { if !v.as_str().map(|s| !s.is_empty()).unwrap_or(false) { return Err(format!("{k} must be a non-empty string")); } }
            "news_sources" => { if !v.as_array().map(|a| a.iter().all(|u| u.is_string())).unwrap_or(false) { return Err("news_sources must be a list of URL strings".into()); } }
            _ => return Err(format!("unknown config key: {k}")),
        }
        out[k] = v.clone();
    }
    Ok(out)
}

// ---------- storage (one connection + lock, same as before) ----------
// shortcut: one connection + lock serialises all runs, upgrade to per-thread connections when parallel out runs matter
struct Db(Mutex<rusqlite::Connection>);
static DB: OnceLock<Db> = OnceLock::new();
fn db() -> &'static Db { DB.get().expect("db init") }

fn row_json(row: &rusqlite::Row) -> rusqlite::Result<Value> {
    let mut m = serde_json::Map::new();
    for (i, name) in row.as_ref().column_names().iter().enumerate() {
        let v = match row.get_ref(i)? {
            rusqlite::types::ValueRef::Null => Value::Null,
            rusqlite::types::ValueRef::Integer(n) => json!(n),
            rusqlite::types::ValueRef::Real(f) => json!(f),
            rusqlite::types::ValueRef::Text(t) => json!(String::from_utf8_lossy(t).into_owned()),
            rusqlite::types::ValueRef::Blob(_) => Value::Null,
        };
        m.insert(name.to_string(), v);
    }
    Ok(Value::Object(m))
}
fn q(sql: &str, args: &[&dyn rusqlite::ToSql]) -> Vec<Value> {
    let conn = db().0.lock().unwrap();
    let mut st = conn.prepare(sql).unwrap();
    let rows = st.query_map(args, row_json).unwrap();
    rows.filter_map(|r| r.ok()).collect()
}
fn q1(sql: &str, args: &[&dyn rusqlite::ToSql]) -> Option<Value> { q(sql, args).into_iter().next() }
fn x(sql: &str, args: &[&dyn rusqlite::ToSql]) -> i64 {
    let conn = db().0.lock().unwrap();
    conn.execute(sql, args).unwrap();
    conn.last_insert_rowid()
}
fn audit(run_id: Option<i64>, event: &str, detail: &str) {
    match run_id { Some(r) => { x("INSERT INTO audit(run_id,event,detail,created_at) VALUES(?,?,?,?)", &[&r, &event, &detail, &now()]); }
        None => { x("INSERT INTO audit(run_id,event,detail,created_at) VALUES(NULL,?,?,?)", &[&event, &detail, &now()]); } }
}

fn init_db(path: &str) {
    let conn = rusqlite::Connection::open(path).unwrap();
    DB.set(Db(Mutex::new(conn))).ok();
    let c = db().0.lock().unwrap();
    c.execute_batch("
        CREATE TABLE IF NOT EXISTS task_types(id INTEGER PRIMARY KEY, name TEXT UNIQUE, template TEXT, mode TEXT DEFAULT 'in', consecutive_success INTEGER DEFAULT 0, failures INTEGER DEFAULT 0);
        CREATE TABLE IF NOT EXISTS runs(id INTEGER PRIMARY KEY, task_type_id INTEGER, input TEXT, mode TEXT, status TEXT, created_at REAL, finished_at REAL);
        CREATE TABLE IF NOT EXISTS steps(id INTEGER PRIMARY KEY, run_id INTEGER, idx INTEGER, name TEXT, status TEXT, output TEXT, confidence REAL, risky INTEGER, pause_reason TEXT, tokens INTEGER DEFAULT 0, cost REAL DEFAULT 0);
        CREATE TABLE IF NOT EXISTS decisions(id INTEGER PRIMARY KEY, run_id INTEGER, step_id INTEGER, action TEXT, edited_output TEXT, created_at REAL);
        CREATE TABLE IF NOT EXISTS audit(id INTEGER PRIMARY KEY, run_id INTEGER, event TEXT, detail TEXT, created_at REAL);
        CREATE TABLE IF NOT EXISTS crons(id INTEGER PRIMARY KEY, name TEXT, schedule_s INTEGER, prompt TEXT, task_type_id INTEGER, enabled INTEGER DEFAULT 1, last_run_at REAL, next_run_at REAL);
        CREATE TABLE IF NOT EXISTS pages(id INTEGER PRIMARY KEY, title TEXT, body TEXT, created_by TEXT, created_at REAL, updated_at REAL);
        CREATE TABLE IF NOT EXISTS settings(id INTEGER PRIMARY KEY, config TEXT);
        CREATE TABLE IF NOT EXISTS proposals(id INTEGER PRIMARY KEY, task_type_id INTEGER, config TEXT, status TEXT, created_at REAL);
        CREATE TABLE IF NOT EXISTS items(id INTEGER PRIMARY KEY, title TEXT, url TEXT, body TEXT, source TEXT, hash TEXT UNIQUE, hype REAL, facts REAL, signal REAL, importance REAL, status TEXT, created_at REAL);
        CREATE TABLE IF NOT EXISTS weights(kind TEXT, key TEXT, weight REAL, PRIMARY KEY(kind, key));
        CREATE TABLE IF NOT EXISTS jev_calls(id INTEGER PRIMARY KEY, run_id INTEGER, command TEXT, model TEXT, input_tokens INTEGER, cost REAL, mock INTEGER, created_at REAL);
    ").unwrap();
    for ddl in ["ALTER TABLE steps ADD COLUMN tokens INTEGER DEFAULT 0", "ALTER TABLE steps ADD COLUMN cost REAL DEFAULT 0"] { let _ = c.execute(ddl, []); } // old DBs predate these columns
    for topic in ["agent", "model", "release"] { c.execute("INSERT OR IGNORE INTO weights(kind,key,weight) VALUES('topic',?,1.0)", [topic]).unwrap(); }
    c.execute("INSERT OR IGNORE INTO settings(id,config) VALUES(1,?)", [default_config().to_string()]).unwrap();
    for name in template_names() { c.execute("INSERT OR IGNORE INTO task_types(name,template) VALUES(?,?)", [name, name]).unwrap(); }
    c.execute("UPDATE runs SET status='killed',finished_at=strftime('%s','now') WHERE status IN ('running','awaiting_decision','paused')", []).unwrap();
    c.execute("UPDATE steps SET status='rejected' WHERE status IN ('pending','awaiting_decision')", []).unwrap();
}

fn get_config() -> Value {
    let mut cfg = default_config();
    if let Some(row) = q1("SELECT config FROM settings WHERE id=1", &[]) {
        if let Ok(saved) = serde_json::from_str::<Value>(row["config"].as_str().unwrap_or("{}")) {
            if let (Some(a), Some(b)) = (cfg.as_object_mut(), saved.as_object()) { for (k, v) in b { a.insert(k.clone(), v.clone()); } }
        }
    }
    cfg
}
fn save_config(patch: &Value) -> Result<Value, String> {
    let clean = validate_config_patch(patch)?;
    let mut cfg = get_config();
    if let (Some(a), Some(b)) = (cfg.as_object_mut(), clean.as_object()) { for (k, v) in b { a.insert(k.clone(), v.clone()); } }
    let s = cfg.to_string();
    x("INSERT OR REPLACE INTO settings(id,config) VALUES(1,?)", &[&s]);
    Ok(cfg)
}
fn eligible(tt: &Value) -> bool {
    let need = match tt["mode"].as_str() { Some("in") => 3, Some("near") => 5, _ => return false };
    tt["consecutive_success"].as_i64().unwrap_or(0) >= need
}
fn tt_view(tt: &Value) -> Value {
    let mut v = tt.clone();
    let mode = tt["mode"].as_str().unwrap_or("in").to_string();
    v["promotion_eligible"] = json!(eligible(tt));
    v["next_mode"] = if mode == "out" { Value::Null } else { json!(MODES[MODES.iter().position(|m| *m == mode).unwrap() + 1]) };
    v
}

// ---------- run control ----------
#[derive(Default, Clone)]
struct CtrlState { kill: bool, paused: bool, decision: Option<Value> }
struct Ctrl { s: Mutex<CtrlState>, cv: Condvar }
static CTRL: OnceLock<Mutex<HashMap<i64, Arc<Ctrl>>>> = OnceLock::new();
fn ctrl_map() -> &'static Mutex<HashMap<i64, Arc<Ctrl>>> { CTRL.get_or_init(|| Mutex::new(HashMap::new())) }
fn ctrl(run_id: i64) -> Arc<Ctrl> {
    ctrl_map().lock().unwrap().entry(run_id).or_insert_with(|| Arc::new(Ctrl { s: Mutex::new(CtrlState::default()), cv: Condvar::new() })).clone()
}
fn ctrl_drop(run_id: i64) { ctrl_map().lock().unwrap().remove(&run_id); }

fn start_run(tt: &Value, text: &str) -> Value {
    let (tid, mode, tname, tmpl) = (tt["id"].as_i64().unwrap(), tt["mode"].as_str().unwrap_or("in").to_string(), tt["name"].as_str().unwrap_or("").to_string(), tt["template"].as_str().unwrap_or("").to_string());
    let rid = x("INSERT INTO runs(task_type_id,input,mode,status,created_at) VALUES(?,?,?,'queued',?)", &[&tid, &text, &mode, &now()]);
    audit(Some(rid), "run_created", &format!("task={tname} mode={mode}"));
    if tmpl == "news" { std::thread::spawn(move || { do_sweep(rid); ctrl_drop(rid); }); }
    else { std::thread::spawn(move || runner(rid)); }
    q1("SELECT * FROM runs WHERE id=?", &[&rid]).unwrap()
}
fn fire_trigger(kind: &str, tt: &Value, text: &str) -> Value {
    let run = start_run(tt, text);
    audit(Some(run["id"].as_i64().unwrap()), "trigger_fired", &format!("kind={kind} task={}", tt["name"].as_str().unwrap_or("")));
    run
}

fn finish(run_id: i64, tt_id: i64, status: &str) {
    x("UPDATE runs SET status=?,finished_at=? WHERE id=?", &[&status, &now(), &run_id]);
    audit(Some(run_id), &format!("run_{status}"), "");
    if status == "completed" { x("UPDATE task_types SET consecutive_success=consecutive_success+1 WHERE id=?", &[&tt_id]); }
    else {
        x("UPDATE task_types SET consecutive_success=0, failures=failures+1 WHERE id=?", &[&tt_id]);
        if q1("SELECT mode FROM task_types WHERE id=?", &[&tt_id]).map(|t| t["mode"].as_str() == Some("out")).unwrap_or(false) {
            x("UPDATE task_types SET mode='near' WHERE id=?", &[&tt_id]);
            audit(Some(run_id), "demoted", "out -> near after failure");
        }
    }
}

fn wait_decision(run_id: i64, step_id: i64, reason: &str) -> Value {
    let c = ctrl(run_id);
    { let mut s = c.s.lock().unwrap(); s.decision = None; }
    x("UPDATE steps SET status='awaiting_decision',pause_reason=? WHERE id=?", &[&reason, &step_id]);
    x("UPDATE runs SET status='awaiting_decision' WHERE id=?", &[&run_id]);
    audit(Some(run_id), "paused", &format!("step {step_id}: {reason}"));
    let deadline = std::time::Instant::now() + Duration::from_secs(RUN_TIMEOUT_S as u64);
    let mut s = c.s.lock().unwrap();
    loop {
        if s.kill || std::time::Instant::now() > deadline { return json!({"action":"kill"}); }
        if let Some(d) = &s.decision { return d.clone(); }
        let (g, _) = c.cv.wait_timeout(s, Duration::from_millis(100)).unwrap(); s = g;
    }
}
fn wait_if_paused(run_id: i64) {
    let c = ctrl(run_id);
    let paused = c.s.lock().unwrap().paused;
    if paused {
        x("UPDATE runs SET status='paused' WHERE id=?", &[&run_id]);
        let mut s = c.s.lock().unwrap();
        while s.paused && !s.kill { s = c.cv.wait(s).unwrap(); }
    }
    if !c.s.lock().unwrap().kill { x("UPDATE runs SET status='running' WHERE id=?", &[&run_id]); }
}

/// Real executor via OpenRouter when a key is set, else stub. Returns (output, confidence, usage).
fn run_step(step_name: &str, text: &str, confidence: f64) -> Result<(String, f64, Option<(i64, f64)>), String> {
    if text.contains("FAIL") { return Err("simulated step failure (FAIL in input)".into()); }
    let cfg = get_config();
    if !cfg["enabled_slices"].as_array().map(|a| a.iter().any(|s| s.as_str() == Some("coding"))).unwrap_or(false) {
        return Err("coding slice disabled (self-modification in effect)".into());
    }
    let key = api_key();
    if key.is_empty() { return Ok((format!("{step_name}: done for '{}'", &text.chars().take(40).collect::<String>()), confidence, None)); }
    let threshold = cfg["conf_threshold"].as_f64().unwrap_or(CONF_THRESHOLD);
    let model = if confidence < threshold { cfg["escalation_model"].as_str().unwrap_or("").to_string() }
        else if confidence >= 0.9 { cfg["cheap_model"].as_str().unwrap_or("").to_string() } else { cfg["model"].as_str().unwrap_or("").to_string() };
    let limit = cfg["context_limit"].as_i64().unwrap_or(4000) as usize;
    let prompt = format!("{}Do this step: {step_name}\nInput:\n{}\n\nReply with ONLY JSON: {{\"output\": \"<result>\", \"confidence\": <0.0-1.0>}}",
        if cfg["system_prompt"].as_str().unwrap_or("").is_empty() { String::new() } else { format!("{}\n", cfg["system_prompt"].as_str().unwrap()) },
        &text.chars().take(limit).collect::<String>());
    let body = ureq::post(&format!("{}/api/v1/chat/completions", openrouter_base()))
        .set("Authorization", &format!("Bearer {key}")).set("X-Title", "loop-app")
        .send_json(json!({"model": model, "temperature": 0.2, "max_tokens": cfg["max_tokens"], "messages": [{"role":"user","content":prompt}]}))
        .map_err(|e| format!("OpenRouter call failed: {e}"))?
        .into_json::<Value>().map_err(|e| format!("OpenRouter call failed: {e}"))?;
    let content = body["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string();
    let usage = &body["usage"];
    let tokens = usage["total_tokens"].as_i64().unwrap_or_else(|| usage["prompt_tokens"].as_i64().unwrap_or(0) + usage["completion_tokens"].as_i64().unwrap_or(0));
    let cost = usage["cost"].as_f64().unwrap_or(0.0);
    let usage = if tokens > 0 { Some((tokens, cost)) } else { None };
    let clean = content.trim().trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```").trim().to_string();
    let parsed: Value = serde_json::from_str(&clean).map_err(|_| format!("OpenRouter returned unparseable output: {content:?}"))?;
    let out = parsed["output"].as_str().filter(|s| !s.is_empty()).ok_or(format!("OpenRouter returned unparseable output: {content:?}"))?.to_string();
    let conf = parsed["confidence"].as_f64().ok_or(format!("OpenRouter returned unparseable output: {content:?}"))?;
    Ok((out, conf.clamp(0.0, 1.0), usage))
}

fn runner(run_id: i64) {
    let run = q1("SELECT * FROM runs WHERE id=?", &[&run_id]).unwrap();
    let tt = q1("SELECT * FROM task_types WHERE id=?", &[&run["task_type_id"].as_i64().unwrap()]).unwrap();
    let (tt_id, mode) = (tt["id"].as_i64().unwrap(), run["mode"].as_str().unwrap_or("in").to_string());
    let c = ctrl(run_id);
    let started = std::time::Instant::now();
    x("UPDATE runs SET status='running' WHERE id=?", &[&run_id]);
    audit(Some(run_id), "run_started", &format!("mode={mode}"));
    let input = run["input"].as_str().unwrap_or("").to_string();
    let mut result = "completed";
    'outer: for (idx, (name, conf, risky)) in template_steps(tt["template"].as_str().unwrap_or("")).iter().enumerate() {
        if c.s.lock().unwrap().kill || started.elapsed().as_secs_f64() > RUN_TIMEOUT_S { result = "killed"; break; }
        wait_if_paused(run_id);
        if c.s.lock().unwrap().kill { result = "killed"; break; }
        let step_id = x("INSERT INTO steps(run_id,idx,name,status,confidence,risky) VALUES(?,?,?,'pending',?,?)", &[&run_id, &(idx as i64), &name, &conf, &(*risky as i64)]);
        let reason = if mode == "in" { Some("approval required (in-the-loop)".to_string()) }
            else if mode == "near" && *conf < get_config()["conf_threshold"].as_f64().unwrap_or(CONF_THRESHOLD) { Some("low confidence".into()) }
            else if mode == "near" && *risky { Some("risky step".into()) } else { None };
        if let Some(reason) = reason {
            let d = wait_decision(run_id, step_id, &reason);
            if d["action"].as_str() == Some("kill") || d["action"].as_str() == Some("reject") {
                x("UPDATE steps SET status='rejected' WHERE id=?", &[&step_id]); result = "killed"; break 'outer;
            }
            x("UPDATE runs SET status='running' WHERE id=?", &[&run_id]);
            if let Some(ed) = d["edited_output"].as_str() {
                x("UPDATE steps SET status='done',output=? WHERE id=?", &[&ed, &step_id]);
                audit(Some(run_id), "step_edited_done", &format!("step {step_id}")); continue 'outer;
            }
        }
        loop {
            match run_step(name, &input, *conf) {
                Ok((out, got, usage)) => {
                    let (tokens, cost) = usage.unwrap_or_else(|| (((input.len() + out.len()) / 4) as i64, 0.0));
                    x("UPDATE steps SET status='done',output=?,confidence=?,tokens=?,cost=? WHERE id=?", &[&out, &got, &tokens, &cost, &step_id]);
                    audit(Some(run_id), "step_done", &format!("step {step_id}: {name}")); break;
                }
                Err(e) => {
                    audit(Some(run_id), "step_failed", &format!("step {step_id}: {e}"));
                    if mode == "out" {
                        x("UPDATE steps SET status='failed' WHERE id=?", &[&step_id]);
                        audit(Some(run_id), "alert", &format!("out-of-loop failure: {e}")); result = "failed"; break 'outer;
                    }
                    let d = wait_decision(run_id, step_id, &format!("failure: {e} (approve=skip step, reject=kill run)"));
                    if d["action"].as_str() == Some("kill") || d["action"].as_str() == Some("reject") {
                        x("UPDATE steps SET status='failed' WHERE id=?", &[&step_id]); result = "failed"; break 'outer;
                    }
                    x("UPDATE steps SET status='skipped',output='skipped after failure by human' WHERE id=?", &[&step_id]);
                    x("UPDATE runs SET status='running' WHERE id=?", &[&run_id]); break;
                }
            }
        }
    }
    finish(run_id, tt_id, result);
    ctrl_drop(run_id);
}

fn resume_queued() {
    for run in q("SELECT * FROM runs WHERE status='queued'", &[]) {
        if let Some(tt) = q1("SELECT * FROM task_types WHERE id=?", &[&run["task_type_id"].as_i64().unwrap()]) {
            let rid = run["id"].as_i64().unwrap();
            audit(Some(rid), "run_resumed", "queued run resumed after restart");
            if tt["template"].as_str() == Some("news") { std::thread::spawn(move || { do_sweep(rid); ctrl_drop(rid); }); }
            else { std::thread::spawn(move || runner(rid)); }
        }
    }
}

// shortcut: cron lives in this process and dies with it; next_run_at is durable in SQLite and recomputed on boot, so a restart delays, never loses.
fn cron_loop() {
    loop {
        std::thread::sleep(Duration::from_secs(1));
        for cron in q("SELECT * FROM crons WHERE enabled=1 AND next_run_at IS NOT NULL AND next_run_at<=?", &[&now()]) {
            let t = now();
            x("UPDATE crons SET last_run_at=?,next_run_at=? WHERE id=?", &[&t, &(t + cron["schedule_s"].as_f64().unwrap_or(0.0)), &cron["id"].as_i64().unwrap()]);
            if let Some(tt) = q1("SELECT * FROM task_types WHERE id=?", &[&cron["task_type_id"].as_i64().unwrap()]) {
                let run = fire_trigger("cron", &tt, cron["prompt"].as_str().unwrap_or(""));
                audit(Some(run["id"].as_i64().unwrap()), "cron_fired", &format!("cron {}: {}", cron["id"], cron["name"].as_str().unwrap_or("")));
            }
        }
    }
}
fn create_cron(data: &Value) -> Result<Value, String> {
    let name = data["name"].as_str().filter(|s| !s.trim().is_empty()).ok_or("cron name and prompt required")?;
    let prompt = data["prompt"].as_str().filter(|s| !s.is_empty()).ok_or("cron name and prompt required")?;
    let sched = data["schedule_s"].as_i64().filter(|s| (1..=86400).contains(s)).ok_or("schedule_s must be 1..86400 (seconds)")?;
    let tt = q1("SELECT * FROM task_types WHERE id=?", &[&data["task_type_id"].as_i64().unwrap_or(-1)]).ok_or("task_type_id not found")?;
    let cid = x("INSERT INTO crons(name,schedule_s,prompt,task_type_id,enabled,last_run_at,next_run_at) VALUES(?,?,?,?,1,NULL,?)",
        &[&name.trim(), &sched, &prompt, &tt["id"].as_i64().unwrap(), &(now() + sched as f64)]);
    Ok(q1("SELECT * FROM crons WHERE id=?", &[&cid]).unwrap())
}
fn create_factory(data: &Value) -> Result<Value, String> {
    let name = data["name"].as_str().filter(|s| !s.trim().is_empty()).ok_or("name required, template must be summarize|classify|report|incident|news")?;
    let template = data["template"].as_str().unwrap_or("summarize");
    if !template_names().contains(&template) { return Err("name required, template must be summarize|classify|report|incident|news".into()); }
    let tid = {
        let conn = db().0.lock().unwrap();
        conn.execute("INSERT INTO task_types(name,template,mode) VALUES(?,?,?)", rusqlite::params![name.trim(), template, "in"])
            .map_err(|_| "name already exists".to_string())?;
        conn.last_insert_rowid()
    };
    let tt = q1("SELECT * FROM task_types WHERE id=?", &[&tid]).unwrap();
    let mut crons = vec![];
    if let Some(list) = data["crons"].as_array() { for c in list { let mut c = c.clone(); c["task_type_id"] = json!(tid); crons.push(create_cron(&c)?); } }
    let mut page = Value::Null;
    if !data["starter_page"].is_null() {
        let sp = &data["starter_page"];
        let title = sp["title"].as_str().filter(|s| !s.trim().is_empty()).ok_or("starter_page needs a title and string body")?;
        if !sp["body"].is_null() && !sp["body"].is_string() { return Err("starter_page needs a title and string body".into()); }
        let pid = x("INSERT INTO pages(title,body,created_by,created_at,updated_at) VALUES(?,?,?,?,?)", &[&title.trim(), &sp["body"].as_str().unwrap_or(""), &"factory-init", &now(), &now()]);
        page = q1("SELECT * FROM pages WHERE id=?", &[&pid]).unwrap();
    }
    audit(None, "factory_created", &format!("{} template={} crons={}", tt["name"].as_str().unwrap_or(""), template, crons.len()));
    Ok(json!({"task_type": tt_view(&tt), "crons": crons, "page": page}))
}

// ---------- Jev (OpenRouter Decisions) ----------
fn jev_dir() -> PathBuf {
    if let Ok(d) = std::env::var("JEV_DIR") { return PathBuf::from(d); }
    let cwd = PathBuf::from("prompt_engineering/jev");
    if cwd.exists() { return cwd; }
    std::env::current_exe().unwrap().parent().unwrap().join("prompt_engineering/jev")
}
fn render(obj: &Value, vars: &Value) -> Value {
    match obj {
        Value::String(s) => {
            if s.starts_with("{{") && s.ends_with("}}") && s.matches("{{").count() == 1 {
                let k = &s[2..s.len() - 2];
                return vars.get(k).cloned().unwrap_or(json!(""));
            }
            let mut out = s.clone();
            if let Some(o) = vars.as_object() { for (k, v) in o { out = out.replace(&format!("{{{{{k}}}}}"), &match v { Value::String(t) => t.clone(), other => other.to_string() }); } }
            json!(out)
        }
        Value::Object(m) => { let mut o = serde_json::Map::new(); for (k, v) in m { o.insert(k.clone(), render(v, vars)); } Value::Object(o) }
        Value::Array(a) => json!(a.iter().map(|v| render(v, vars)).collect::<Vec<_>>()),
        other => other.clone(),
    }
}
fn load_jev(name: &str) -> Result<Value, String> {
    let p = jev_dir().join(name).join("command.json");
    let s = std::fs::read_to_string(&p).map_err(|e| format!("jev command {name} unreadable: {e}"))?;
    serde_json::from_str(&s).map_err(|e| format!("jev command {name} unreadable: {e}"))
}
fn jev_mock_answers(state: &Value, questions: &Value) -> Value {
    let text = if state.is_object() { format!("{} {}", state["title"].as_str().unwrap_or(""), state["excerpt"].as_str().unwrap_or("")).to_lowercase() } else { state.to_string().to_lowercase() };
    let hot = ["release", "available", "agent", "model", "flag"].iter().any(|w| text.contains(w));
    let mut answers = serde_json::Map::new();
    for (qid, qu) in questions.as_object().cloned().unwrap_or_default() {
        let a = match qu["type"].as_str().unwrap_or("") {
            "noul" => json!({"type":"noul","noul": if hot {0.9} else {0.15}}),
            "choice" => { let opts: Vec<String> = qu["criteria"].as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
                json!({"type":"choice","choice": opts.first().cloned().unwrap_or_default(),"probabilities":{},"confidence":0.5}) }
            _ => { let crit = qu["criteria"].as_array().cloned().unwrap_or_else(|| vec![json!(1), json!(2)]);
                let top = (crit.len() as f64) - 1.0; let mut pos = if hot {0.85} else {0.2};
                if qid == "hype" { pos = if text.contains("changes everything") || text.contains("revolutionary") {0.85} else {0.25}; }
                if qid == "facts" { pos = if text.chars().any(|c| c.is_ascii_digit()) || text.contains("version") {0.85} else {0.3}; }
                let legend: serde_json::Map<String, Value> = crit.iter().enumerate().map(|(i, c)| (i.to_string(), c.clone())).collect();
                json!({"type":"score","score": pos * top,"legend": legend,"probabilities":{},"confidence":0.5}) }
        };
        answers.insert(qid, a);
    }
    Value::Object(answers)
}
fn jev_classify(state: &Value, questions: &Value, command: &str, run_id: Option<i64>) -> Result<Value, String> {
    if jev_mock() {
        let a = jev_mock_answers(state, questions);
        match run_id { Some(r) => { x("INSERT INTO jev_calls(run_id,command,model,input_tokens,cost,mock,created_at) VALUES(?,?,?,0,0,1,?)", &[&r, &command, &"mock", &now()]); } None => { x("INSERT INTO jev_calls(run_id,command,model,input_tokens,cost,mock,created_at) VALUES(NULL,?, ?,0,0,1,?)", &[&command, &"mock", &now()]); } }
        return Ok(a);
    }
    let key = api_key();
    if key.is_empty() { return Err("jev unavailable (no OPENROUTER_API_KEY); set JEV_MOCK=1 for explicit dev mock".into()); }
    let cfg = get_config();
    let url = format!("{}/api/alpha/decisions", openrouter_base());
    let mut last = String::new();
    for attempt in 0..3 {
        let resp = ureq::post(&url).set("Authorization", &format!("Bearer {key}")).set("X-Title", "loop-app")
            .send_json(json!({"model": cfg["jev_model"], "state": state, "questions": questions}));
        match resp {
            Ok(r) => {
                let data: Value = r.into_json().map_err(|e| format!("jev call failed: {e}"))?;
                let answers = &data["answers"];
                if !answers.is_object() || !answers.as_object().unwrap().values().all(|a| a.is_object() && a.get("type").is_some()) { return Err("jev call failed: malformed jev answers".into()); }
                let tokens = data["usage"]["input_tokens"].as_i64().unwrap_or(0);
                let cost = data["usage"]["cost"].as_f64().unwrap_or(tokens as f64 / 1_000_000.0 * JEV_PRICE_PER_M);
                let model = data["model"].as_str().unwrap_or(cfg["jev_model"].as_str().unwrap_or("")).to_string();
                match run_id { Some(rid) => { x("INSERT INTO jev_calls(run_id,command,model,input_tokens,cost,mock,created_at) VALUES(?,?,?,?,?,0,?)", &[&rid, &command, &model, &tokens, &cost, &now()]); } None => { x("INSERT INTO jev_calls(run_id,command,model,input_tokens,cost,mock,created_at) VALUES(NULL,?,?,?,?,0,?)", &[&command, &model, &tokens, &cost, &now()]); } }
                return Ok(answers.clone());
            }
            Err(ureq::Error::Status(code, _)) => { last = format!("HTTP {code}"); if ![429, 502, 503, 529].contains(&code) { break; } std::thread::sleep(Duration::from_millis(500 * (attempt + 1))); }
            Err(e) => return Err(format!("jev call failed: {e}")),
        }
    }
    Err(format!("jev call failed: {last}"))
}
fn run_jev(name: &str, vars: &Value, run_id: Option<i64>) -> Result<(Value, Value), String> {
    let cmd = load_jev(name)?;
    let questions = render(&cmd["questions"], vars);
    let answers = jev_classify(&render(&cmd["state"], vars), &questions, name, run_id)?;
    Ok((answers, cmd["policy"].clone()))
}
fn score100(a: &Value) -> f64 {
    let top = a["legend"].as_object().map(|o| o.len()).unwrap_or(0).max(a["probabilities"].as_object().map(|o| o.len()).unwrap_or(0));
    let top = if top == 0 { 2 } else { top } as f64 - 1.0;
    if top > 0.0 { (a["score"].as_f64().unwrap_or(0.0) / top).clamp(0.0, 1.0) * 100.0 } else { 0.0 }
}
fn topic_keywords() -> Vec<Value> { q("SELECT * FROM weights WHERE kind='topic' ORDER BY key", &[]).iter().map(|w| json!({"key": w["key"], "weight": w["weight"]})).collect() }
fn taste_examples() -> Value {
    json!({"liked": q("SELECT title FROM items WHERE status='presented' ORDER BY id DESC LIMIT 3", &[]).iter().map(|i| i["title"].clone()).collect::<Vec<_>>(),
           "disliked": q("SELECT title FROM items WHERE status='dismissed' ORDER BY id DESC LIMIT 3", &[]).iter().map(|i| i["title"].clone()).collect::<Vec<_>>()})
}
fn item_hash(title: &str, body: &str) -> String { let mut h = Sha256::new(); h.update(format!("{title}\n{body}").as_bytes()); format!("{:x}", h.finalize())[..32].to_string() }
fn insert_item(title: &str, url: &str, body: &str, source: &str) -> (i64, bool) {
    let h = item_hash(title, body);
    let conn = db().0.lock().unwrap();
    match conn.execute("INSERT INTO items(title,url,body,source,hash,status,created_at) VALUES(?,?,?,?,?,'new',?)", rusqlite::params![title, url, body, source, h, now()]) {
        Ok(_) => (conn.last_insert_rowid(), true),
        Err(_) => { drop(conn); (q1("SELECT id FROM items WHERE hash=?", &[&h]).map(|r| r["id"].as_i64().unwrap()).unwrap_or(0), false) }
    }
}
fn fetch_source(url: &str) -> Result<Vec<(String, String, String)>, String> {
    let resp = ureq::get(url).set("User-Agent", "loop-app news sweep").timeout(Duration::from_secs(10)).call().map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    resp.into_reader().take(5_000_000).read_to_end(&mut bytes).map_err(|e| e.to_string())?; // shortcut: feeds over 5 MB fail parse and are skipped per-source, raise the cap when a real source outgrows it
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let doc = roxmltree::Document::parse(&text).map_err(|e| e.to_string())?;
    let mut out = vec![];
    for node in doc.descendants() {
        if node.tag_name().name() == "item" || node.tag_name().name() == "entry" {
            let get = |n: &str| node.descendants().find(|c| c.tag_name().name() == n).and_then(|c| c.text()).unwrap_or("").trim().to_string();
            let title = get("title");
            if !title.is_empty() { out.push((title, { let l = get("link"); if l.is_empty() { url.to_string() } else { l } }, { let d = get("description"); if d.is_empty() { get("summary") } else { d } })); }
        }
    }
    Ok(out)
}
fn do_sweep(run_id: i64) {
    let run = q1("SELECT * FROM runs WHERE id=?", &[&run_id]).unwrap();
    let tt = q1("SELECT * FROM task_types WHERE id=?", &[&run["task_type_id"].as_i64().unwrap()]).unwrap();
    let tt_id = tt["id"].as_i64().unwrap();
    x("UPDATE runs SET status='running' WHERE id=?", &[&run_id]);
    audit(Some(run_id), "run_started", &format!("mode={} template=news", run["mode"].as_str().unwrap_or("")));
    let mut idx = -1i64;
    macro_rules! phase { ($name:expr, $output:expr) => {{ idx += 1; x("INSERT INTO steps(run_id,idx,name,status,output,confidence,risky,tokens,cost) VALUES(?,?,?,'done',?,0.95,0,0,0)", &[&run_id, &idx, &$name, &$output]); audit(Some(run_id), "step_done", &format!("{}: {}", $name, $output)); }}; }
    let result: Result<(), String> = (|| {
        let mut new_count = 0i64;
        if let Some(sources) = get_config()["news_sources"].as_array().cloned() {
            for u in sources { let url = u.as_str().unwrap_or("").to_string();
                match fetch_source(&url) { Ok(items) => { for (t, l, d) in items { let (_, created) = insert_item(&t, &l, &d, &url); new_count += created as i64; } } Err(e) => audit(Some(run_id), "source_skipped", &format!("{url}: {e}")), }
            }
        }
        phase!("gather", format!("{new_count} new items"));
        let mut gated: Vec<Value> = vec![]; let mut folded = 0i64;
        let presented: std::collections::HashSet<String> = q("SELECT hash FROM items WHERE status='presented'", &[]).iter().map(|i| i["hash"].as_str().unwrap_or("").to_string()).collect();
        for item in q("SELECT * FROM items WHERE status='new' ORDER BY id LIMIT 50", &[]) {
            if presented.contains(item["hash"].as_str().unwrap_or("")) { x("UPDATE items SET status='dup' WHERE id=?", &[&item["id"].as_i64().unwrap()]); folded += 1; continue; }
            let topics = topic_keywords();
            let mut choices = serde_json::Map::new(); for w in &topics { choices.insert(w["key"].as_str().unwrap_or("").to_string(), Value::Null); }
            if choices.is_empty() { choices.insert("agent".into(), Value::Null); }
            let vars = json!({"title": item["title"], "source": item["source"], "excerpt": item["body"].as_str().unwrap_or("").chars().take(800).collect::<String>(), "published_at": "", "topic_keywords": topics, "taste_examples": taste_examples(), "topic_choices": Value::Object(choices)});
            let (rel, rel_pol) = run_jev("relevance", &vars, Some(run_id))?;
            let (worth, worth_pol) = run_jev("worth-attention", &vars, Some(run_id))?;
            if score100(&rel["relevance"]) / 100.0 < rel_pol["relevance_min"].as_f64().unwrap_or(0.45) || worth["worth"]["noul"].as_f64().unwrap_or(0.0) < worth_pol["worth_min"].as_f64().unwrap_or(0.35) {
                x("UPDATE items SET status='gated' WHERE id=?", &[&item["id"].as_i64().unwrap()]);
            } else { gated.push(item); }
        }
        phase!("gate", format!("{} passed, {} hash-dupes folded", gated.len(), folded));
        let mut kept: Vec<Value> = vec![];
        for item in gated {
            let dupe = kept.iter().find(|k: &&Value| k["title"].as_str().unwrap_or("").to_lowercase() == item["title"].as_str().unwrap_or("").to_lowercase()).cloned();
            if let Some(d) = dupe { // shortcut: jev same-story only when titles collide; hash+title cover the common case
                let (ans, pol) = run_jev("same-story", &json!({"candidate": item["title"], "existing": d["title"]}), Some(run_id))?;
                if ans["same"]["noul"].as_f64().unwrap_or(0.0) >= pol["same"].as_f64().unwrap_or(0.85) { x("UPDATE items SET status='dup' WHERE id=?", &[&item["id"].as_i64().unwrap()]); continue; }
            }
            kept.push(item);
        }
        phase!("dedupe", format!("{} unique stories", kept.len()));
        let topics = topic_keywords();
        for item in kept.iter_mut() {
            let excerpt: String = item["body"].as_str().unwrap_or("").chars().take(800).collect();
            let (hfs, _) = run_jev("hfs", &json!({"title": item["title"], "source": item["source"], "excerpt": excerpt, "findings": "", "section_keywords": {}, "taste_examples": taste_examples()}), Some(run_id))?;
            let (imp, _) = run_jev("importance", &json!({"title": item["title"], "source": item["source"], "source_weight": 1, "published_at": "", "excerpt": excerpt, "topic": "", "topic_weight": 1, "topic_keywords": topics, "taste_examples": taste_examples()}), Some(run_id))?;
            let text = format!("{} {}", item["title"].as_str().unwrap_or(""), item["body"].as_str().unwrap_or("")).to_lowercase();
            let tw = topics.iter().filter(|w| text.contains(w["key"].as_str().unwrap_or(""))).map(|w| w["weight"].as_f64().unwrap_or(1.0)).fold(None::<f64>, |a, b| Some(a.map_or(b, |x: f64| x.max(b)))).unwrap_or(1.0);
            let sw = q1("SELECT weight FROM weights WHERE kind='source' AND key=?", &[&item["source"].as_str().unwrap_or("")]).map(|r| r["weight"].as_f64().unwrap_or(1.0)).unwrap_or(1.0);
            let age_days = ((now() - item["created_at"].as_f64().unwrap_or(now())) / 86400.0).max(0.0);
            let rank = score100(&imp["importance"]) * tw * sw / (1.0 + age_days);
            let (h, f, s) = (score100(&hfs["hype"]), score100(&hfs["facts"]), score100(&hfs["signal"]));
            x("UPDATE items SET hype=?,facts=?,signal=?,importance=?,status='ranked' WHERE id=?", &[&h, &f, &s, &rank, &item["id"].as_i64().unwrap()]);
            item["hype"] = json!(h); item["facts"] = json!(f); item["signal"] = json!(s); item["importance"] = json!(rank);
        }
        kept.sort_by(|a, b| b["importance"].as_f64().unwrap_or(0.0).partial_cmp(&a["importance"].as_f64().unwrap_or(0.0)).unwrap());
        phase!("rank", format!("ranked {}", kept.len()));
        let top: Vec<Value> = kept.into_iter().take(7).collect();
        for item in &top {
            x("UPDATE items SET status='presented' WHERE id=?", &[&item["id"].as_i64().unwrap()]);
            if item["hype"].as_f64().unwrap_or(0.0) >= 50.0 && item["signal"].as_f64().unwrap_or(0.0) >= 50.0 && item["facts"].as_f64().unwrap_or(0.0) < 50.0 {
                audit(Some(run_id), "suspect_combo", &format!("item {}: hype+signal without facts — needs human taste", item["id"]));
            }
        }
        let cards: String = top.iter().map(|i| format!("<article><h3>{}</h3><p>Hype {} · Facts {} · Signal {}</p><p>{}</p></article>", i["title"].as_str().unwrap_or(""), i["hype"], i["facts"], i["signal"], i["body"].as_str().unwrap_or("").chars().take(300).collect::<String>())).collect();
        let title = format!("News sweep {}", {
            let secs = now() as i64 % 86400; format!("{:02}:{:02}", secs / 3600, (secs % 3600) / 60)
        });
        let pid = x("INSERT INTO pages(title,body,created_by,created_at,updated_at) VALUES(?,?,?,?,?)", &[&title, &if cards.is_empty() { "<p>nothing new</p>".to_string() } else { cards }, &"agent", &now(), &now()]);
        phase!("present", format!("{} cards on page {}", top.len(), pid));
        Ok(())
    })();
    match result {
        Ok(()) => finish(run_id, tt_id, "completed"),
        Err(e) => { // a sweep must fail its run, never strand it running
            audit(Some(run_id), "sweep_failed", &e);
            if run["mode"].as_str() == Some("out") { audit(Some(run_id), "alert", &format!("out-of-loop sweep failure: {e}")); }
            finish(run_id, tt_id, "failed");
        }
    }
}

// ---------- HTTP surface ----------
fn tools_manifest() -> Value {
    let t: Vec<Value> = [
        ("list_task_types","GET","/api/task-types","task list"),("create_task_type","POST","/api/task-types","mode/promote controls"),
        ("set_mode","POST","/api/task-types/:id/mode","mode segmented control"),("promote","POST","/api/task-types/:id/promote","Promote button"),
        ("list_runs","GET","/api/runs","run list"),("start_run","POST","/api/runs","Run button"),("get_run","GET","/api/runs/:id","selected run view"),
        ("decide","POST","/api/runs/:id/decide","Approve / Reject buttons"),("control","POST","/api/runs/:id/control","pause/resume/kill buttons"),
        ("delete_run","DELETE","/api/runs/:id","delete button"),("list_audit","GET","/api/audit","audit trail"),
        ("list_crons","GET","/api/crons","cron list"),("create_cron","POST","/api/crons","cron form"),("delete_cron","DELETE","/api/crons/:id","cron delete button"),
        ("list_pages","GET","/api/pages","page list"),("create_page","POST","/api/pages","page form"),("edit_page","POST","/api/pages/:id","page form"),
        ("delete_page","DELETE","/api/pages/:id","page delete button"),("get_config","GET","/api/config","agent panel config"),
        ("update_config","PUT","/api/config","agent panel config (human only)"),("propose_config","POST","/api/proposals","agent self-modification (needs human decide)"),
        ("list_proposals","GET","/api/proposals","proposal list"),("decide_proposal","POST","/api/proposals/:id/decide","proposal Approve / Reject"),
        ("create_factory","POST","/api/factories","factory init form / --init-factory CLI"),("list_tools","GET","/api/tools","agent panel tools"),
        ("trigger_webhook","POST","/api/triggers/:task_type_id","webhook (no UI control — external trigger)"),
        ("ingest_event","POST","/api/events","log-event stream (external trigger)"),("ingest_item","POST","/api/items","news gather (sweep also fetches sources)"),
        ("list_items","GET","/api/items","news feed"),("vote_item","POST","/api/items/:id/vote","card up/down vote"),
        ("list_weights","GET","/api/weights","taste weights"),("set_weight","POST","/api/weights","taste weight editor"),
        ("executions","GET","/api/executions","agent panel executions"),
    ].iter().map(|(n, m, p, u)| json!({"name": n, "method": m, "path": p, "ui": u})).collect();
    let mut sl = serde_json::Map::new(); for (k, v) in slices() { sl.insert(k.into(), json!(v)); }
    json!({"tools": t, "slices": Value::Object(sl)})
}

fn header_json() -> tiny_http::Header { tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap() }
fn respond(req: tiny_http::Request, code: u16, body: Value) {
    let _ = req.respond(tiny_http::Response::from_string(body.to_string()).with_status_code(code).with_header(header_json()));
}
fn authorized(req: &tiny_http::Request) -> bool {
    let token = std::env::var("LOOP_TOKEN").unwrap_or_default();
    if token.is_empty() { return true; }
    req.headers().iter().any(|h| (h.field.as_str() == "X-Loop-Token" && h.value.as_str() == token) || (h.field.as_str() == "Authorization" && h.value.as_str() == format!("Bearer {token}")))
}
fn read_body(req: &mut tiny_http::Request) -> Result<Value, ()> {
    let mut buf = Vec::new();
    req.as_reader().take((MAX_BODY + 1) as u64).read_to_end(&mut buf).map_err(|_| ())?;
    if buf.len() > MAX_BODY { return Err(()); }
    if buf.is_empty() { return Ok(json!({})); }
    serde_json::from_slice(&buf).map_err(|_| ())
}
fn sid(s: &str) -> i64 { s.parse().unwrap_or(-1) }

fn handle(mut req: tiny_http::Request) {
    let url = req.url().to_string();
    let path = url.split('?').next().unwrap_or("").to_string();
    let parts: Vec<String> = path.split('/').filter(|p| !p.is_empty()).map(|p| p.to_string()).collect();
    let method = req.method().as_str().to_string();
    if parts.first().map(|p| p == "api").unwrap_or(false) && !authorized(&req) { return respond(req, 401, json!({"error":"unauthorized"})); }

    if method == "GET" {
        match parts.iter().map(|s| s.as_str()).collect::<Vec<_>>().as_slice() {
            [] => {
                let p = if PathBuf::from("static/index.html").exists() { PathBuf::from("static/index.html") } else { std::env::current_exe().unwrap().parent().unwrap().join("static/index.html") };
                let page = std::fs::read(&p).unwrap_or_default();
                let h = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html"[..]).unwrap();
                let _ = req.respond(tiny_http::Response::from_data(page).with_header(h)); return;
            }
            ["api", "task-types"] => return respond(req, 200, json!(q("SELECT * FROM task_types ORDER BY id", &[]).iter().map(tt_view).collect::<Vec<_>>())),
            ["api", "runs"] => return respond(req, 200, json!(q("SELECT * FROM runs ORDER BY id DESC LIMIT 50", &[]))),
            ["api", "runs", id] => {
                return match q1("SELECT * FROM runs WHERE id=?", &[&sid(id)]) {
                    Some(mut run) => { run["steps"] = json!(q("SELECT * FROM steps WHERE run_id=? ORDER BY idx", &[&run["id"].as_i64().unwrap()])); run["audit"] = json!(q("SELECT * FROM audit WHERE run_id=? ORDER BY id", &[&run["id"].as_i64().unwrap()])); respond(req, 200, run) }
                    None => respond(req, 404, json!({"error":"run not found"})),
                };
            }
            ["api", "audit"] => return respond(req, 200, json!(q("SELECT * FROM audit ORDER BY id DESC LIMIT 100", &[]))),
            ["api", "tools"] => return respond(req, 200, tools_manifest()),
            ["api", "config"] => return respond(req, 200, get_config()),
            ["api", "crons"] => return respond(req, 200, json!(q("SELECT * FROM crons ORDER BY id", &[]))),
            ["api", "pages"] => return respond(req, 200, json!(q("SELECT * FROM pages ORDER BY id", &[]))),
            ["api", "pages", id] => return match q1("SELECT * FROM pages WHERE id=?", &[&sid(id)]) { Some(p) => respond(req, 200, p), None => respond(req, 404, json!({"error":"page not found"})) },
            ["api", "proposals"] => return respond(req, 200, json!(q("SELECT * FROM proposals ORDER BY id DESC LIMIT 50", &[]))),
            ["api", "items"] => {
                let status = url.split("status=").nth(1).map(|s| s.split('&').next().unwrap_or("").to_string());
                let rows = match &status { Some(s) => q("SELECT * FROM items WHERE status=? ORDER BY importance DESC, id DESC LIMIT 50", &[&s.as_str()]), None => q("SELECT * FROM items ORDER BY id DESC LIMIT 50", &[]) };
                return respond(req, 200, json!(rows));
            }
            ["api", "weights"] => return respond(req, 200, json!(q("SELECT * FROM weights ORDER BY kind, key", &[]))),
            ["api", "executions"] => {
                let steps = q1("SELECT COALESCE(SUM(tokens),0) AS t, COALESCE(SUM(cost),0) AS c FROM steps", &[]).unwrap_or(json!({"t":0,"c":0}));
                let jev = q1("SELECT COUNT(*) AS n, COALESCE(SUM(cost),0) AS c, COALESCE(SUM(mock),0) AS m FROM jev_calls", &[]).unwrap_or(json!({"n":0,"c":0,"m":0}));
                let live = q1("SELECT COUNT(*) AS n FROM runs WHERE status IN ('queued','running','awaiting_decision','paused')", &[]).unwrap()["n"].as_i64().unwrap_or(0);
                let crons_on = q1("SELECT COUNT(*) AS n FROM crons WHERE enabled=1", &[]).unwrap()["n"].as_i64().unwrap_or(0);
                let mut by = serde_json::Map::new(); for r in q("SELECT status, COUNT(*) AS n FROM runs GROUP BY status", &[]) { by.insert(r["status"].as_str().unwrap_or("").to_string(), r["n"].clone()); }
                return respond(req, 200, json!({"step_tokens": steps["t"], "step_cost": steps["c"], "jev_calls": jev["n"], "jev_cost": jev["c"], "jev_mock_calls": jev["m"],
                    "agents_working_now": live + crons_on, "live_runs": live, "enabled_crons": crons_on, "runs_by_status": Value::Object(by)}));
            }
            _ => return respond(req, 404, json!({"error":"not found"})),
        }
    }

    if method == "DELETE" {
        match parts.iter().map(|s| s.as_str()).collect::<Vec<_>>().as_slice() {
            ["api", "runs", id] => {
                return match q1("SELECT * FROM runs WHERE id=?", &[&sid(id)]) {
                    None => respond(req, 404, json!({"error":"run not found"})),
                    Some(run) => {
                        if !["completed", "killed", "failed"].contains(&run["status"].as_str().unwrap_or("")) { return respond(req, 400, json!({"error":"only finished runs can be deleted"})); }
                        let rid = run["id"].as_i64().unwrap();
                        for t in ["steps", "decisions", "audit"] { x(&format!("DELETE FROM {t} WHERE run_id=?"), &[&rid]); }
                        x("DELETE FROM runs WHERE id=?", &[&rid]); ctrl_drop(rid);
                        audit(None, "run_deleted", &format!("run {} task_type={} status={}", rid, run["task_type_id"], run["status"].as_str().unwrap_or("")));
                        respond(req, 200, json!({"ok": true, "deleted": rid}))
                    }
                };
            }
            ["api", "crons", id] => return match q1("SELECT * FROM crons WHERE id=?", &[&sid(id)]) {
                None => respond(req, 404, json!({"error":"cron not found"})),
                Some(c) => { x("DELETE FROM crons WHERE id=?", &[&c["id"].as_i64().unwrap()]); audit(None, "cron_deleted", &format!("cron {}: {}", c["id"], c["name"].as_str().unwrap_or(""))); respond(req, 200, json!({"ok": true, "deleted": c["id"].as_i64().unwrap()})) }
            },
            ["api", "pages", id] => return match q1("SELECT * FROM pages WHERE id=?", &[&sid(id)]) {
                None => respond(req, 404, json!({"error":"page not found"})),
                Some(p) => { x("DELETE FROM pages WHERE id=?", &[&p["id"].as_i64().unwrap()]); audit(None, "page_deleted", &format!("page {}: {}", p["id"], p["title"].as_str().unwrap_or(""))); respond(req, 200, json!({"ok": true, "deleted": p["id"].as_i64().unwrap()})) }
            },
            _ => return respond(req, 404, json!({"error":"not found"})),
        }
    }

    if method == "PUT" {
        if parts.iter().map(|s| s.as_str()).collect::<Vec<_>>() == ["api", "config"] {
            return match read_body(&mut req) { Ok(data) if data.is_object() => match save_config(&data) { Ok(c) => respond(req, 200, c), Err(e) => respond(req, 400, json!({"error": e})) }, _ => respond(req, 400, json!({"error":"invalid JSON or Content-Length"})) };
        }
        return respond(req, 404, json!({"error":"not found"}));
    }

    if method != "POST" { return respond(req, 404, json!({"error":"not found"})); }
    let data = match read_body(&mut req) { Ok(d) if d.is_object() => d, _ => return respond(req, 400, json!({"error":"invalid JSON or Content-Length"})) };
    let p: Vec<&str> = parts.iter().map(|s| s.as_str()).collect();

    match p.as_slice() {
        ["api", "task-types"] => {
            let name = data["name"].as_str().unwrap_or(""); let template = data["template"].as_str().unwrap_or("summarize"); let mode = data["mode"].as_str().unwrap_or("in");
            if name.trim().is_empty() || !template_names().contains(&template) || !MODES.contains(&mode) {
                return respond(req, 400, json!({"error":"name required, template must be summarize|classify|report|incident|news, mode must be in|near|out"}));
            }
            let conn = db().0.lock().unwrap();
            match conn.execute("INSERT INTO task_types(name,template,mode) VALUES(?,?,?)", rusqlite::params![name.trim(), template, mode]) {
                Ok(_) => { let id = conn.last_insert_rowid(); drop(conn); respond(req, 201, tt_view(&q1("SELECT * FROM task_types WHERE id=?", &[&id]).unwrap())) }
                Err(_) => respond(req, 400, json!({"error":"name already exists"})),
            }
        }
        ["api", "task-types", id, "mode"] => {
            match q1("SELECT * FROM task_types WHERE id=?", &[&sid(id)]) { None => respond(req, 404, json!({"error":"task type not found"})),
                Some(tt) => { let m = data["mode"].as_str().unwrap_or(""); if !MODES.contains(&m) { return respond(req, 400, json!({"error":"mode must be in|near|out"})); }
                    x("UPDATE task_types SET mode=? WHERE id=?", &[&m, &tt["id"].as_i64().unwrap()]); respond(req, 200, tt_view(&q1("SELECT * FROM task_types WHERE id=?", &[&tt["id"].as_i64().unwrap()]).unwrap())) } }
        }
        ["api", "task-types", id, "promote"] => {
            match q1("SELECT * FROM task_types WHERE id=?", &[&sid(id)]) { None => respond(req, 404, json!({"error":"task type not found"})),
                Some(tt) => {
                    if tt["mode"].as_str() == Some("out") { return respond(req, 400, json!({"error":"already out-of-the-loop"})); }
                    if !(data["force"].as_bool().unwrap_or(false) || eligible(&tt)) { return respond(req, 400, json!({"error":"not eligible yet (or send force:true)"})); }
                    let cur = tt["mode"].as_str().unwrap_or("in"); let nxt = MODES[MODES.iter().position(|m| *m == cur).unwrap() + 1];
                    x("UPDATE task_types SET mode=?,consecutive_success=0 WHERE id=?", &[&nxt, &tt["id"].as_i64().unwrap()]);
                    audit(None, "promoted", &format!("{}: {} -> {}", tt["name"].as_str().unwrap_or(""), cur, nxt));
                    respond(req, 200, tt_view(&q1("SELECT * FROM task_types WHERE id=?", &[&tt["id"].as_i64().unwrap()]).unwrap()))
                } }
        }
        ["api", "runs"] => {
            match q1("SELECT * FROM task_types WHERE id=?", &[&data["task_type_id"].as_i64().unwrap_or(-1)]) { None => respond(req, 400, json!({"error":"task_type_id not found"})),
                Some(tt) => { if data.get("input").map(|v| !v.is_string()).unwrap_or(false) { return respond(req, 400, json!({"error":"input must be a string"})); }
                    respond(req, 201, start_run(&tt, data["input"].as_str().unwrap_or(""))) } }
        }
        ["api", "crons"] => match create_cron(&data) { Ok(c) => respond(req, 201, c), Err(e) => respond(req, 400, json!({"error": e})) },
        ["api", "pages"] => {
            let title = data["title"].as_str().unwrap_or("");
            if title.trim().is_empty() || (!data["body"].is_null() && !data["body"].is_string()) { return respond(req, 400, json!({"error":"page title required, body must be a string"})); }
            let pid = x("INSERT INTO pages(title,body,created_by,created_at,updated_at) VALUES(?,?,?,?,?)", &[&title.trim(), &data["body"].as_str().unwrap_or(""), &data["created_by"].as_str().unwrap_or("human"), &now(), &now()]);
            audit(None, "page_created", &format!("page {}: {}", pid, title.trim()));
            respond(req, 201, q1("SELECT * FROM pages WHERE id=?", &[&pid]).unwrap())
        }
        ["api", "pages", id] => {
            match q1("SELECT * FROM pages WHERE id=?", &[&sid(id)]) { None => respond(req, 404, json!({"error":"page not found"})),
                Some(page) => { let title = if data["title"].is_null() { page["title"].as_str().unwrap_or("").to_string() } else { data["title"].as_str().unwrap_or("").to_string() };
                    let body = if data["body"].is_null() { page["body"].as_str().unwrap_or("").to_string() } else { data["body"].as_str().unwrap_or("").to_string() };
                    if title.trim().is_empty() { return respond(req, 400, json!({"error":"page title required, body must be a string"})); }
                    x("UPDATE pages SET title=?,body=?,updated_at=? WHERE id=?", &[&title.trim(), &body, &now(), &page["id"].as_i64().unwrap()]);
                    audit(None, "page_edited", &format!("page {}: {}", page["id"], title.trim()));
                    respond(req, 200, q1("SELECT * FROM pages WHERE id=?", &[&page["id"].as_i64().unwrap()]).unwrap()) } }
        }
        ["api", "factories"] => match create_factory(&data) { Ok(f) => respond(req, 201, f), Err(e) => respond(req, 400, json!({"error": e})) },
        ["api", "proposals"] => {
            match q1("SELECT * FROM task_types WHERE id=?", &[&data["task_type_id"].as_i64().unwrap_or(-1)]) { None => respond(req, 400, json!({"error":"task_type_id not found"})),
                Some(tt) => { if tt["mode"].as_str() == Some("out") { return respond(req, 400, json!({"error":"out-of-the-loop agents cannot propose self-modification"})); }
                    match validate_config_patch(&data["config"]) { Err(e) => respond(req, 400, json!({"error": e})),
                        Ok(patch) => { let keys: Vec<String> = patch.as_object().unwrap().keys().cloned().collect();
                            let pid = x("INSERT INTO proposals(task_type_id,config,status,created_at) VALUES(?,?,'pending',?)", &[&tt["id"].as_i64().unwrap(), &patch.to_string(), &now()]);
                            audit(None, "proposal_created", &format!("proposal {} task={}: {:?}", pid, tt["name"].as_str().unwrap_or(""), keys));
                            respond(req, 201, q1("SELECT * FROM proposals WHERE id=?", &[&pid]).unwrap()) } } } }
        }
        ["api", "proposals", id, "decide"] => {
            match q1("SELECT * FROM proposals WHERE id=?", &[&sid(id)]) { None => respond(req, 404, json!({"error":"proposal not found"})),
                Some(prop) => { if prop["status"].as_str() != Some("pending") { return respond(req, 400, json!({"error":"proposal already decided"})); }
                    let act = data["action"].as_str().unwrap_or("");
                    if act == "approve" { let patch: Value = serde_json::from_str(prop["config"].as_str().unwrap_or("{}")).unwrap_or(json!({})); let _ = save_config(&patch);
                        x("UPDATE proposals SET status='applied' WHERE id=?", &[&prop["id"].as_i64().unwrap()]); audit(None, "self_modified", &format!("proposal {} applied: {}", prop["id"], prop["config"].as_str().unwrap_or(""))); }
                    else if act == "reject" { x("UPDATE proposals SET status='rejected' WHERE id=?", &[&prop["id"].as_i64().unwrap()]); audit(None, "proposal_rejected", &format!("proposal {}", prop["id"])); }
                    else { return respond(req, 400, json!({"error":"action must be approve|reject"})); }
                    respond(req, 200, q1("SELECT * FROM proposals WHERE id=?", &[&prop["id"].as_i64().unwrap()]).unwrap()) } }
        }
        ["api", "triggers", id] => {
            match q1("SELECT * FROM task_types WHERE id=?", &[&sid(id)]) { None => respond(req, 404, json!({"error":"task type not found"})),
                Some(tt) => { if data.get("input").map(|v| !v.is_string()).unwrap_or(false) { return respond(req, 400, json!({"error":"input must be a string"})); }
                    respond(req, 201, fire_trigger("webhook", &tt, data["input"].as_str().unwrap_or(""))) } }
        }
        ["api", "events"] => {
            let status = data["status"].as_i64(); let message = data["message"].as_str();
            if status.is_none() || data["status"].is_boolean() || message.is_none() { return respond(req, 400, json!({"error":"status (int) and message (string) required"})); }
            let (status, message) = (status.unwrap(), message.unwrap().to_string());
            let tt = q1("SELECT * FROM task_types WHERE id=?", &[&data["task_type_id"].as_i64().unwrap_or(-1)])
                .or_else(|| q1("SELECT * FROM task_types WHERE template='incident' ORDER BY id", &[]));
            match tt { None => respond(req, 400, json!({"error":"no incident task type; create one first"})),
                Some(tt) => {
                    if (400..600).contains(&status) { return respond(req, 201, json!({"started": true, "run": fire_trigger("event", &tt, &format!("[{status}] {message}"))})); }
                    let topics = topic_keywords(); let mut choices = serde_json::Map::new(); for w in &topics { choices.insert(w["key"].as_str().unwrap_or("").to_string(), Value::Null); }
                    if choices.is_empty() { choices.insert("agent".into(), Value::Null); }
                    match run_jev("relevance", &json!({"title": message, "source": data["source"].as_str().unwrap_or("event"), "excerpt": message, "topic_keywords": topics, "taste_examples": taste_examples(), "topic_choices": Value::Object(choices)}), None) {
                        Err(e) => respond(req, 503, json!({"error": e})),
                        Ok((rel, pol)) => { let relevant = score100(&rel["relevance"]) / 100.0 >= pol["relevance_min"].as_f64().unwrap_or(0.45);
                            audit(None, "event_received", &format!("status={} relevant={}: {}", status, relevant, message.chars().take(80).collect::<String>()));
                            if !relevant { respond(req, 200, json!({"started": false, "reason": "classified not relevant"})) }
                            else { respond(req, 201, json!({"started": true, "run": fire_trigger("event", &tt, &format!("[{status}] {message}"))})) } }
                    }
                } }
        }
        ["api", "items"] => {
            let title = data["title"].as_str().unwrap_or("");
            if title.trim().is_empty() || (!data["body"].is_null() && !data["body"].is_string()) { return respond(req, 400, json!({"error":"item title required, body must be a string"})); }
            let (iid, created) = insert_item(title.trim(), data["url"].as_str().unwrap_or(""), data["body"].as_str().unwrap_or(""), data["source"].as_str().unwrap_or("manual"));
            respond(req, if created { 201 } else { 200 }, q1("SELECT * FROM items WHERE id=?", &[&iid]).unwrap_or(json!({})))
        }
        ["api", "items", id, "vote"] => {
            match q1("SELECT * FROM items WHERE id=?", &[&sid(id)]) { None => respond(req, 404, json!({"error":"item not found"})),
                Some(item) => { let vote = data["vote"].as_str().unwrap_or("");
                    if vote != "up" && vote != "down" { return respond(req, 400, json!({"error":"vote must be up|down"})); }
                    let delta = if vote == "up" { 0.1 } else { -0.1 };
                    if vote == "down" { x("UPDATE items SET status='dismissed' WHERE id=?", &[&item["id"].as_i64().unwrap()]); }
                    let src = item["source"].as_str().unwrap_or("").to_string();
                    let cur = q1("SELECT weight FROM weights WHERE kind='source' AND key=?", &[&src.as_str()]).map(|r| r["weight"].as_f64().unwrap_or(1.0)).unwrap_or(1.0);
                    x("INSERT OR REPLACE INTO weights(kind,key,weight) VALUES('source',?,?)", &[&src.as_str(), &((cur + delta).max(0.1))]);
                    let text = format!("{} {}", item["title"].as_str().unwrap_or(""), item["body"].as_str().unwrap_or("")).to_lowercase();
                    for w in topic_keywords() { if text.contains(w["key"].as_str().unwrap_or("")) { x("INSERT OR REPLACE INTO weights(kind,key,weight) VALUES('topic',?,?)", &[&w["key"].as_str().unwrap_or(""), &((w["weight"].as_f64().unwrap_or(1.0) + delta).max(0.1))]); } }
                    audit(None, &format!("item_{vote}voted"), &format!("item {}: {}", item["id"], item["title"].as_str().unwrap_or("").chars().take(60).collect::<String>()));
                    respond(req, 200, q1("SELECT * FROM items WHERE id=?", &[&item["id"].as_i64().unwrap()]).unwrap()) } }
        }
        ["api", "weights"] => {
            let kind = data["kind"].as_str().unwrap_or(""); let key = data["key"].as_str().unwrap_or(""); let weight = data["weight"].as_f64();
            if !["topic", "source"].contains(&kind) || key.trim().is_empty() || weight.map(|w| !(0.1..=2.0).contains(&w)).unwrap_or(true) || data["weight"].is_boolean() {
                return respond(req, 400, json!({"error":"kind topic|source, key, weight 0.1..2 required"}));
            }
            x("INSERT OR REPLACE INTO weights(kind,key,weight) VALUES(?,?,?)", &[&kind, &key.trim(), &weight.unwrap()]);
            respond(req, 200, json!({"kind": kind, "key": key.trim(), "weight": weight.unwrap()}))
        }
        ["api", "runs", id, "decide"] => {
            match q1("SELECT * FROM runs WHERE id=?", &[&sid(id)]) { None => respond(req, 404, json!({"error":"run not found"})),
                Some(run) => { let act = data["action"].as_str().unwrap_or("");
                    if act != "approve" && act != "reject" { return respond(req, 400, json!({"error":"action must be approve|reject"})); }
                    match q1("SELECT * FROM steps WHERE run_id=? AND status='awaiting_decision' ORDER BY idx", &[&run["id"].as_i64().unwrap()]) { None => respond(req, 400, json!({"error":"no step awaiting decision"})),
                        Some(step) => { let edited: Option<&str> = data["edited_output"].as_str();
                            match edited { Some(e) => { x("INSERT INTO decisions(run_id,step_id,action,edited_output,created_at) VALUES(?,?,?,?,?)", &[&run["id"].as_i64().unwrap(), &step["id"].as_i64().unwrap(), &act, &e, &now()]); } None => { x("INSERT INTO decisions(run_id,step_id,action,edited_output,created_at) VALUES(?,?,?,?,?)", &[&run["id"].as_i64().unwrap(), &step["id"].as_i64().unwrap(), &act, &rusqlite::types::Value::Null, &now()]); } }
                            audit(Some(run["id"].as_i64().unwrap()), &format!("decision_{act}"), &format!("step {}", step["id"]));
                            let c = ctrl(run["id"].as_i64().unwrap()); { let mut s = c.s.lock().unwrap(); let mut d = json!({"action": act}); if let Some(e) = edited { d["edited_output"] = json!(e); } s.decision = Some(d); } c.cv.notify_all();
                            respond(req, 200, json!({"ok": true, "action": act, "step_id": step["id"].as_i64().unwrap()})) } } } }
        }
        ["api", "runs", id, "control"] => {
            match q1("SELECT * FROM runs WHERE id=?", &[&sid(id)]) { None => respond(req, 404, json!({"error":"run not found"})),
                Some(run) => { let act = data["action"].as_str().unwrap_or(""); let c = ctrl(run["id"].as_i64().unwrap());
                    match act { "pause" => c.s.lock().unwrap().paused = true, "resume" => c.s.lock().unwrap().paused = false,
                        "kill" => { let mut s = c.s.lock().unwrap(); s.kill = true; } _ => return respond(req, 400, json!({"error":"action must be pause|resume|kill"})) }
                    c.cv.notify_all(); audit(Some(run["id"].as_i64().unwrap()), &format!("control_{act}"), "");
                    respond(req, 200, json!({"ok": true, "action": act})) } }
        }
        _ => respond(req, 404, json!({"error":"not found"})),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let db_path = env_or("LOOP_DB", "loop.db");
    init_db(&db_path);
    if args.get(1).map(|a| a == "--init-factory").unwrap_or(false) {
        let data: Value = serde_json::from_str(&std::fs::read_to_string(&args[2]).unwrap()).unwrap();
        match create_factory(&data) { Ok(f) => println!("{}", serde_json::to_string_pretty(&f).unwrap()), Err(e) => { eprintln!("{e}"); std::process::exit(1); } }
        return;
    }
    resume_queued();
    std::thread::spawn(cron_loop);
    let port: u16 = std::env::var("PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(8000);
    let host = env_or("HOST", "localhost");
    let server = tiny_http::Server::http(format!("{host}:{port}")).unwrap();
    println!("loop app on http://localhost:{port}");
    for req in server.incoming_requests() { std::thread::spawn(move || handle(req)); }
}
