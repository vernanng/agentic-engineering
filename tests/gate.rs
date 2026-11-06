//! End-to-end gate through the real HTTP surface. Boots the real binary on
//! port 18099 with a temp DB; OpenRouter and Jev wire are mocked locally. No spend.
use serde_json::{json, Value};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const BASE: &str = "http://localhost:18099";
const MOCK: &str = "http://localhost:18123";

fn call(method: &str, path: &str, data: Option<Value>, expect: Option<u16>) -> Value {
    let url = format!("{BASE}{path}");
    let req = match method {
        "GET" => ureq::get(&url),
        "POST" => ureq::post(&url),
        "PUT" => ureq::put(&url),
        "DELETE" => ureq::delete(&url),
        _ => unreachable!(),
    };
    let resp = match data {
        Some(d) => req.send_json(d),
        None => req.call(),
    };
    let (code, out) = match resp {
        Ok(r) => {
            let c = r.status();
            (c, r.into_json().unwrap_or(json!({})))
        }
        Err(ureq::Error::Status(c, r)) => (c, r.into_json().unwrap_or(json!({}))),
        Err(e) => panic!("{method} {path}: {e}"),
    };
    if let Some(e) = expect {
        assert_eq!(code, e, "{method} {path} -> {code} {out}");
    }
    out
}
fn wait_run(rid: i64, status: &str) -> Value {
    let end = Instant::now() + Duration::from_secs(8);
    while Instant::now() < end {
        let r = call("GET", &format!("/api/runs/{rid}"), None, None);
        if r["status"].as_str() == Some(status) {
            return r;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    panic!(
        "run {rid} never reached {status}: {}",
        call("GET", &format!("/api/runs/{rid}"), None, None)
    );
}

static JEV_CALLS: AtomicUsize = AtomicUsize::new(0);
fn mock_server() {
    std::thread::spawn(|| {
        let server = tiny_http::Server::http("localhost:18123").unwrap();
        let flaky = tiny_http::Server::http("localhost:18124").unwrap();
        std::thread::spawn(move || {
            for mut req in flaky.incoming_requests() {
                let n = JEV_CALLS.fetch_add(1, Ordering::SeqCst);
                let mut b = String::new();
                let _ = req.as_reader().read_to_string(&mut b);
                if n == 0 {
                    let _ =
                        req.respond(tiny_http::Response::from_string("busy").with_status_code(503));
                    continue;
                }
                let _ = req.respond(tiny_http::Response::from_string(json!({"model":"jev-test","answers":{"worth":{"type":"noul","noul":0.9}},"usage":{"input_tokens":1000}}).to_string())
                    .with_header(tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap()));
            }
        });
        for mut req in server.incoming_requests() {
            let url = req.url().to_string();
            let mut b = String::new();
            let _ = req.as_reader().read_to_string(&mut b);
            let h = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                .unwrap();
            if url.contains("chat/completions") {
                let content = if b.contains("not json") {
                    "not json".to_string()
                } else {
                    json!({"output":"real summary","confidence":0.9}).to_string()
                };
                let _ = req.respond(tiny_http::Response::from_string(json!({"choices":[{"message":{"content":content}}],"usage":{"prompt_tokens":900,"completion_tokens":99,"total_tokens":999,"cost":0.5}}).to_string()).with_header(h));
            } else if url.contains("decisions") {
                let _ = req.respond(tiny_http::Response::from_string(json!({"model":"jev-test","answers":{"worth":{"type":"noul","noul":0.9}},"usage":{"input_tokens":1000}}).to_string()).with_header(h));
            } else {
                let _ = req.respond(tiny_http::Response::from_string("{}").with_status_code(404));
            }
        }
    });
}

struct Srv(Child);
impl Drop for Srv {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

fn boot(db: &str, extra: &[(&str, &str)]) -> Srv {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_loop-app"));
    cmd.env("PORT", "18099")
        .env("LOOP_DB", db)
        .env("JEV_MOCK", "1")
        .env("OPENROUTER_API_BASE", MOCK)
        .env_remove("OPENROUTER_API_KEY")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (k, v) in extra {
        cmd.env(k, v);
    }
    let child = cmd.spawn().unwrap();
    for _ in 0..50 {
        if let Ok(r) = ureq::get(&format!("{BASE}/api/task-types")).call() {
            if r.status() == 200 {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Srv(child)
}

#[test]
fn gate() {
    mock_server();
    // Jev wire via a dedicated binary instance pointed at the flaky mock (503 retry, fail-closed).
    let db1 = std::env::temp_dir().join(format!("gate-wire-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db1);
    {
        let _srv = boot(
            db1.to_str().unwrap(),
            &[
                ("OPENROUTER_API_KEY", "test-key"),
                ("OPENROUTER_API_BASE", "http://localhost:18124"),
                ("JEV_MOCK", ""),
            ],
        );
        // decisions-backed event path retries once on 503 (indirect: 200-status event starts or gates without 503 surfacing)
        let ev = call(
            "POST",
            "/api/events",
            Some(json!({"status":200,"message":"routine health ping"})),
            None,
        );
        assert!(ev.get("started").is_some(), "{ev}");
    }
    // Cheap tier honors OPENROUTER_CODING_MODEL, else falls back to OPENROUTER_MODEL.
    let dbc = std::env::temp_dir().join(format!("gate-cheap-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&dbc);
    {
        let _srv = boot(
            dbc.to_str().unwrap(),
            &[
                ("OPENROUTER_API_KEY", "test-key"),
                ("OPENROUTER_MODEL", "openai/gpt-4o-mini"),
                ("OPENROUTER_CODING_MODEL", "openai/gpt-4o"),
            ],
        );
        let cfg = call("GET", "/api/config", None, None);
        assert_eq!(cfg["cheap_model"], json!("openai/gpt-4o"), "{cfg}");
    }
    let dbc = std::env::temp_dir().join(format!("gate-cheapfb-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&dbc);
    {
        let _srv = boot(
            dbc.to_str().unwrap(),
            &[
                ("OPENROUTER_API_KEY", "test-key"),
                ("OPENROUTER_MODEL", "openai/gpt-4o-mini"),
            ],
        );
        let cfg = call("GET", "/api/config", None, None);
        assert_eq!(cfg["cheap_model"], json!("openai/gpt-4o-mini"), "{cfg}");
    }
    // Exec backend: fake `muse` executable (rustc-built fixture, no shell in
    // repo). A run completes with the fixture's JSON as the step output.
    // Skip when rustc is missing (some runners lack a toolchain); the
    // /bin/false block below still covers the exec error path.
    let has_rustc = Command::new("rustc")
        .arg("--version")
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let exec_dir = std::env::temp_dir().join(format!("gate-exec-{}", std::process::id()));
    std::fs::create_dir_all(&exec_dir).unwrap();
    std::fs::write(
        exec_dir.join("fake-muse.rs"),
        r#"fn main(){println!("{{\"output\":\"exec ok\",\"confidence\":0.9}}");}"#,
    )
    .unwrap();
    let fake = if has_rustc {
        let fake = exec_dir.join("fake-muse");
        assert!(Command::new("rustc")
            .arg("-o")
            .arg(&fake)
            .arg(exec_dir.join("fake-muse.rs"))
            .status()
            .unwrap()
            .success());
        Some(fake)
    } else {
        println!("rustc missing, skipping exec fixture");
        None
    };
    let dbe = std::env::temp_dir().join(format!("gate-exec-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&dbe);
    if let Some(fake) = fake {
    {
        let _srv = boot(
            dbe.to_str().unwrap(),
            &[
                ("LOOP_EXEC_BACKEND", "1"),
                ("MUSE_BIN", fake.to_str().unwrap()),
            ],
        );
        let tt = call(
            "POST",
            "/api/task-types",
            Some(json!({"name":"check-exec","template":"summarize","mode":"out"})),
            Some(201),
        );
        let run = call(
            "POST",
            "/api/runs",
            Some(json!({"task_type_id":tt["id"],"input":"ok"})),
            Some(201),
        );
        let r = wait_run(run["id"].as_i64().unwrap(), "completed");
        let steps = r["steps"].as_array().unwrap();
        assert!(!steps.is_empty(), "{r}");
        assert!(
            steps.iter().all(|s| s["output"] == json!("exec ok")),
            "{r}"
        );
        println!("exec backend ok");
    }
    }
    // Exec backend failure (non-zero exit) fails the run with an alert, like
    // the FAIL hook does for the default backend.
    let dbef = std::env::temp_dir().join(format!("gate-execfail-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&dbef);
    {
        let _srv = boot(
            dbef.to_str().unwrap(),
            &[("LOOP_EXEC_BACKEND", "1"), ("MUSE_BIN", "/bin/false")],
        );
        let tt = call(
            "POST",
            "/api/task-types",
            Some(json!({"name":"check-execfail","template":"summarize","mode":"out"})),
            Some(201),
        );
        let run = call(
            "POST",
            "/api/runs",
            Some(json!({"task_type_id":tt["id"],"input":"ok"})),
            Some(201),
        );
        let r = wait_run(run["id"].as_i64().unwrap(), "failed");
        assert!(r["audit"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["event"] == "alert"));
        println!("exec backend failure ok");
    }
    // MSP backend with an unspawnable host fails the run instead of hanging.
    let dbm = std::env::temp_dir().join(format!("gate-mspfail-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&dbm);
    {
        let _srv = boot(
            dbm.to_str().unwrap(),
            &[
                ("LOOP_MSP_BACKEND", "1"),
                ("MUSE_BIN", "/nonexistent-muse-xyz"),
            ],
        );
        let tt = call(
            "POST",
            "/api/task-types",
            Some(json!({"name":"check-mspfail","template":"summarize","mode":"out"})),
            Some(201),
        );
        let run = call(
            "POST",
            "/api/runs",
            Some(json!({"task_type_id":tt["id"],"input":"ok"})),
            Some(201),
        );
        let r = wait_run(run["id"].as_i64().unwrap(), "failed");
        assert!(r["audit"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["event"] == "alert"));
        println!("msp spawn failure ok");
    }
    let db = std::env::temp_dir().join(format!("gate-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let srv = boot(db.to_str().unwrap(), &[("OPENROUTER_API_KEY", "test-key")]);

    // Runs list must never stick on "loading…": page carries a token field and
    // shows fetch errors (e.g. missing X-Loop-Token) in place of the spinner.
    let page = ureq::get(&format!("{BASE}/"))
        .call()
        .unwrap()
        .into_string()
        .unwrap();
    assert!(page.contains("id=\"token\""), "token input missing");
    assert!(
        page.contains("could not load runs"),
        "refresh error text missing"
    );

    call(
        "POST",
        "/api/task-types",
        Some(json!({"name":"check-badmode","template":"summarize","mode":"bogus"})),
        Some(400),
    );
    // 1. in-the-loop
    let tt = call(
        "POST",
        "/api/task-types",
        Some(json!({"name":"check-in","template":"summarize","mode":"in"})),
        Some(201),
    );
    let run = call(
        "POST",
        "/api/runs",
        Some(json!({"task_type_id":tt["id"],"input":"hello"})),
        Some(201),
    );
    let r = wait_run(run["id"].as_i64().unwrap(), "awaiting_decision");
    assert!(r["steps"].as_array().unwrap().len() == 1);
    call(
        "POST",
        &format!("/api/runs/{}/decide", run["id"]),
        Some(json!({"action":"approve"})),
        Some(200),
    );
    wait_run(run["id"].as_i64().unwrap(), "awaiting_decision");
    call(
        "POST",
        &format!("/api/runs/{}/decide", run["id"]),
        Some(json!({"action":"approve","edited_output":"human final"})),
        Some(200),
    );
    let r = wait_run(run["id"].as_i64().unwrap(), "completed");
    assert_eq!(r["steps"][1]["output"], json!("human final"));
    // real usage ledger: mocked chat reports 999 tokens / $0.5 per step
    assert_eq!(r["steps"][0]["tokens"], json!(999), "{}", r["steps"]);
    assert_eq!(r["steps"][0]["cost"], json!(0.5), "{}", r["steps"]);
    let run = call(
        "POST",
        "/api/runs",
        Some(json!({"task_type_id":tt["id"],"input":"hello"})),
        Some(201),
    );
    wait_run(run["id"].as_i64().unwrap(), "awaiting_decision");
    call(
        "POST",
        &format!("/api/runs/{}/decide", run["id"]),
        Some(json!({"action":"reject"})),
        Some(200),
    );
    wait_run(run["id"].as_i64().unwrap(), "killed");
    println!("in-the-loop + usage ledger ok");

    // 2. near-the-loop
    let tt2 = call(
        "POST",
        "/api/task-types",
        Some(json!({"name":"check-near","template":"summarize","mode":"near"})),
        Some(201),
    );
    let run = call(
        "POST",
        "/api/runs",
        Some(json!({"task_type_id":tt2["id"],"input":"clean"})),
        Some(201),
    );
    wait_run(run["id"].as_i64().unwrap(), "completed");
    let tt3 = call(
        "POST",
        "/api/task-types",
        Some(json!({"name":"check-near2","template":"classify","mode":"near"})),
        Some(201),
    );
    let run = call(
        "POST",
        "/api/runs",
        Some(json!({"task_type_id":tt3["id"],"input":"ticket"})),
        Some(201),
    );
    let r = wait_run(run["id"].as_i64().unwrap(), "awaiting_decision");
    assert!(
        r["steps"][0]["pause_reason"]
            .as_str()
            .unwrap()
            .contains("low confidence"),
        "{}",
        r["steps"]
    );
    call(
        "POST",
        &format!("/api/runs/{}/decide", run["id"]),
        Some(json!({"action":"approve"})),
        Some(200),
    );
    let r = wait_run(run["id"].as_i64().unwrap(), "awaiting_decision");
    assert!(
        r["steps"][1]["pause_reason"]
            .as_str()
            .unwrap()
            .contains("risky"),
        "{}",
        r["steps"]
    );
    call(
        "POST",
        &format!("/api/runs/{}/decide", run["id"]),
        Some(json!({"action":"approve"})),
        Some(200),
    );
    wait_run(run["id"].as_i64().unwrap(), "completed");
    println!("near-the-loop ok");

    // 3. out-of-the-loop
    let tt4 = call(
        "POST",
        "/api/task-types",
        Some(json!({"name":"check-out","template":"summarize","mode":"out"})),
        Some(201),
    );
    let run = call(
        "POST",
        "/api/runs",
        Some(json!({"task_type_id":tt4["id"],"input":"FAIL please"})),
        Some(201),
    );
    let r = wait_run(run["id"].as_i64().unwrap(), "failed");
    assert!(r["audit"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["event"] == "alert"));
    let after = call("GET", "/api/task-types", None, None)
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == tt4["id"])
        .unwrap()
        .clone();
    assert_eq!(after["mode"], json!("near"));
    println!("out-of-the-loop ok");

    // 4. promotion
    let tt5 = call(
        "POST",
        "/api/task-types",
        Some(json!({"name":"check-promo","template":"summarize","mode":"in"})),
        Some(201),
    );
    for _ in 0..3 {
        let run = call(
            "POST",
            "/api/runs",
            Some(json!({"task_type_id":tt5["id"],"input":"ok"})),
            Some(201),
        );
        for _ in 0..2 {
            wait_run(run["id"].as_i64().unwrap(), "awaiting_decision");
            call(
                "POST",
                &format!("/api/runs/{}/decide", run["id"]),
                Some(json!({"action":"approve"})),
                Some(200),
            );
        }
        wait_run(run["id"].as_i64().unwrap(), "completed");
    }
    let me = call("GET", "/api/task-types", None, None)
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == tt5["id"])
        .unwrap()
        .clone();
    assert_eq!(me["promotion_eligible"], json!(true), "{me}");
    let promoted = call(
        "POST",
        &format!("/api/task-types/{}/promote", tt5["id"]),
        Some(json!({})),
        Some(200),
    );
    assert_eq!(promoted["mode"], json!("near"));
    println!("promotion ok");

    // 5. delete finished only
    let live = call(
        "POST",
        "/api/runs",
        Some(json!({"task_type_id":tt["id"],"input":"ok"})),
        Some(201),
    );
    wait_run(live["id"].as_i64().unwrap(), "awaiting_decision");
    call(
        "DELETE",
        &format!("/api/runs/{}", live["id"]),
        None,
        Some(400),
    );
    call(
        "POST",
        &format!("/api/runs/{}/decide", live["id"]),
        Some(json!({"action":"reject"})),
        Some(200),
    );
    wait_run(live["id"].as_i64().unwrap(), "killed");
    call(
        "DELETE",
        &format!("/api/runs/{}", live["id"]),
        None,
        Some(200),
    );
    call("GET", &format!("/api/runs/{}", live["id"]), None, Some(404));
    println!("delete ok");

    // 6. validation
    call(
        "POST",
        "/api/task-types",
        Some(json!({"name":123,"template":"summarize"})),
        Some(400),
    );
    let n_before = call("GET", "/api/runs", None, None)
        .as_array()
        .unwrap()
        .len();
    call(
        "POST",
        "/api/runs",
        Some(json!({"task_type_id":tt["id"],"input":null})),
        Some(400),
    );
    assert_eq!(
        call("GET", "/api/runs", None, None)
            .as_array()
            .unwrap()
            .len(),
        n_before
    );
    println!("validation ok");

    // 7. TSF: tools/config/pages/proposals/slices/cron/factory
    let manifest = call("GET", "/api/tools", None, None);
    let names: Vec<&str> = manifest["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for n in [
        "start_run",
        "decide",
        "create_cron",
        "create_page",
        "propose_config",
        "create_factory",
    ] {
        assert!(names.contains(&n), "{names:?}");
    }
    assert_eq!(
        manifest["slices"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>()
            .len(),
        4
    );
    call(
        "PUT",
        "/api/config",
        Some(json!({"bogus_key":1})),
        Some(400),
    );
    call(
        "PUT",
        "/api/config",
        Some(json!({"thinking_level":"high"})),
        Some(200),
    );
    assert_eq!(
        call("GET", "/api/config", None, None)["thinking_level"],
        json!("high")
    );
    let page = call(
        "POST",
        "/api/pages",
        Some(json!({"title":"Check Page","body":"<b>hi</b>","created_by":"agent"})),
        Some(201),
    );
    let edited = call(
        "POST",
        &format!("/api/pages/{}", page["id"]),
        Some(json!({"body":"<i>edited</i>"})),
        Some(200),
    );
    assert_eq!(edited["body"], json!("<i>edited</i>"));
    call(
        "DELETE",
        &format!("/api/pages/{}", page["id"]),
        None,
        Some(200),
    );
    let prop = call(
        "POST",
        "/api/proposals",
        Some(json!({"task_type_id":tt["id"],"config":{"thinking_level":"max"}})),
        Some(201),
    );
    assert_eq!(
        call("GET", "/api/config", None, None)["thinking_level"],
        json!("high")
    );
    call(
        "POST",
        &format!("/api/proposals/{}/decide", prop["id"]),
        Some(json!({"action":"approve"})),
        Some(200),
    );
    assert_eq!(
        call("GET", "/api/config", None, None)["thinking_level"],
        json!("max")
    );
    call(
        "POST",
        &format!("/api/proposals/{}/decide", prop["id"]),
        Some(json!({"action":"approve"})),
        Some(400),
    );
    let tt_out = call(
        "POST",
        "/api/task-types",
        Some(json!({"name":"check-out2","template":"summarize","mode":"out"})),
        Some(201),
    );
    call(
        "POST",
        "/api/proposals",
        Some(json!({"task_type_id":tt_out["id"],"config":{"thinking_level":"low"}})),
        Some(400),
    );
    call(
        "PUT",
        "/api/config",
        Some(json!({"enabled_slices":["factory","pages","factory-controls"]})),
        Some(200),
    );
    let run = call(
        "POST",
        "/api/runs",
        Some(json!({"task_type_id":tt2["id"],"input":"slice check"})),
        Some(201),
    );
    let r = wait_run(run["id"].as_i64().unwrap(), "awaiting_decision");
    assert!(
        r["steps"][0]["pause_reason"]
            .as_str()
            .unwrap_or("")
            .contains("coding slice disabled"),
        "{}",
        r["steps"]
    );
    call(
        "POST",
        &format!("/api/runs/{}/decide", run["id"]),
        Some(json!({"action":"reject"})),
        Some(200),
    );
    wait_run(run["id"].as_i64().unwrap(), "failed");
    call(
        "PUT",
        "/api/config",
        Some(json!({"enabled_slices":["factory","coding","pages","factory-controls"]})),
        Some(200),
    );
    let ttc = call(
        "POST",
        "/api/task-types",
        Some(json!({"name":"check-cron","template":"summarize","mode":"near"})),
        Some(201),
    );
    let cron = call(
        "POST",
        "/api/crons",
        Some(
            json!({"name":"check-tick","schedule_s":1,"prompt":"cron hello","task_type_id":ttc["id"]}),
        ),
        Some(201),
    );
    let end = Instant::now() + Duration::from_secs(12);
    let mut fired = Value::Null;
    while Instant::now() < end && fired.is_null() {
        fired = call("GET", "/api/audit", None, None)
            .as_array()
            .unwrap()
            .iter()
            .find(|a| {
                a["event"] == "cron_fired"
                    && a["detail"]
                        .as_str()
                        .unwrap_or("")
                        .contains(&format!("cron {}", cron["id"]))
            })
            .cloned()
            .unwrap_or(Value::Null);
        if fired.is_null() {
            std::thread::sleep(Duration::from_millis(300));
        }
    }
    assert!(!fired.is_null(), "cron never fired");
    wait_run(fired["run_id"].as_i64().unwrap(), "completed");
    call(
        "DELETE",
        &format!("/api/crons/{}", cron["id"]),
        None,
        Some(200),
    );
    let fac = call(
        "POST",
        "/api/factories",
        Some(
            json!({"name":"check-factory","template":"summarize","crons":[{"name":"f-cron","schedule_s":3600,"prompt":"hello"}],"starter_page":{"title":"Start","body":"<b>hi</b>"}}),
        ),
        Some(201),
    );
    assert_eq!(fac["task_type"]["mode"], json!("in"));
    call(
        "DELETE",
        &format!("/api/crons/{}", fac["crons"][0]["id"]),
        None,
        Some(200),
    );
    // factory CLI parity
    let cfg_path = std::env::temp_dir().join("factory.json");
    std::fs::write(
        &cfg_path,
        json!({"name":"check-factory-cli","template":"report"}).to_string(),
    )
    .unwrap();
    let cli_db = std::env::temp_dir().join("gate-cli.db");
    let _ = std::fs::remove_file(&cli_db);
    let out = Command::new(env!("CARGO_BIN_EXE_loop-app"))
        .args(["--init-factory", cfg_path.to_str().unwrap()])
        .env("LOOP_DB", &cli_db)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["task_type"]["mode"],
        json!("in")
    );
    println!("tsf ok");

    // 8. leaving the loop: webhook, incident, news sweep, flywheel
    let run = call(
        "POST",
        &format!("/api/triggers/{}", tt2["id"]),
        Some(json!({"input":"via webhook"})),
        Some(201),
    );
    let r = wait_run(run["id"].as_i64().unwrap(), "completed");
    assert!(r["audit"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["event"] == "trigger_fired"));
    let tti = call(
        "POST",
        "/api/task-types",
        Some(json!({"name":"check-incident","template":"incident","mode":"near"})),
        Some(201),
    );
    let ev = call(
        "POST",
        "/api/events",
        Some(json!({"status":503,"message":"db pool exhausted","task_type_id":tti["id"]})),
        Some(201),
    );
    assert_eq!(ev["started"], json!(true));
    let r = wait_run(ev["run"]["id"].as_i64().unwrap(), "awaiting_decision");
    assert_eq!(
        r["steps"].as_array().unwrap().last().unwrap()["name"],
        json!("apply fix")
    );
    call(
        "POST",
        &format!("/api/runs/{}/decide", ev["run"]["id"]),
        Some(json!({"action":"approve"})),
        Some(200),
    );
    wait_run(ev["run"]["id"].as_i64().unwrap(), "completed");
    let ev2 = call(
        "POST",
        "/api/events",
        Some(json!({"status":200,"message":"routine health ping","task_type_id":tti["id"]})),
        Some(200),
    );
    assert_eq!(ev2["started"], json!(false), "{ev2}");
    let ttn = call(
        "POST",
        "/api/task-types",
        Some(json!({"name":"check-news","template":"news","mode":"near"})),
        Some(201),
    );
    let good = call(
        "POST",
        "/api/items",
        Some(
            json!({"title":"Agent framework release v2 available now","body":"The agent model release v2 is available now with a new flag.","source":"wire-a"}),
        ),
        Some(201),
    );
    call(
        "POST",
        "/api/items",
        Some(
            json!({"title":"Agent framework release v2 available now","body":"The agent model release v2 is available now with a new flag.","source":"wire-a"}),
        ),
        Some(200),
    );
    let bad = call(
        "POST",
        "/api/items",
        Some(
            json!({"title":"Celebrity giveaway scandal shocks fans","body":"A celebrity giveaway scandal with no engineering content.","source":"wire-b"}),
        ),
        Some(201),
    );
    let run = call(
        "POST",
        "/api/runs",
        Some(json!({"task_type_id":ttn["id"],"input":"sweep"})),
        Some(201),
    );
    wait_run(run["id"].as_i64().unwrap(), "completed");
    let items: std::collections::HashMap<i64, Value> = call("GET", "/api/items", None, None)
        .as_array()
        .unwrap()
        .iter()
        .map(|i| (i["id"].as_i64().unwrap(), i.clone()))
        .collect();
    assert_eq!(
        items[&good["id"].as_i64().unwrap()]["status"],
        json!("presented"),
        "{}",
        items[&good["id"].as_i64().unwrap()]
    );
    assert_eq!(
        items[&bad["id"].as_i64().unwrap()]["status"],
        json!("gated"),
        "{}",
        items[&bad["id"].as_i64().unwrap()]
    );
    let ex = call("GET", "/api/executions", None, None);
    assert!(
        ex["jev_calls"].as_i64().unwrap() > 0 && ex["jev_mock_calls"] == ex["jev_calls"],
        "{ex}"
    );
    let before: std::collections::HashMap<String, f64> = call("GET", "/api/weights", None, None)
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["kind"] == "source")
        .map(|w| {
            (
                w["key"].as_str().unwrap().to_string(),
                w["weight"].as_f64().unwrap(),
            )
        })
        .collect();
    call(
        "POST",
        &format!("/api/items/{}/vote", good["id"]),
        Some(json!({"vote":"up"})),
        Some(200),
    );
    let after: std::collections::HashMap<String, f64> = call("GET", "/api/weights", None, None)
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["kind"] == "source")
        .map(|w| {
            (
                w["key"].as_str().unwrap().to_string(),
                w["weight"].as_f64().unwrap(),
            )
        })
        .collect();
    assert!(
        after.get("wire-a").copied().unwrap_or(1.0) > before.get("wire-a").copied().unwrap_or(1.0),
        "{before:?} {after:?}"
    );
    call(
        "POST",
        &format!("/api/items/{}/vote", good["id"]),
        Some(json!({"vote":"down"})),
        Some(200),
    );
    println!("leaving-the-loop ok");

    // Risk fix: Rust reads a copy of the real Python-era DB; CLI parity on the same server.
    drop(srv);
    std::thread::sleep(Duration::from_millis(300));
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("loop.db");
    if src.exists() {
        let copy = std::env::temp_dir().join(format!("gate-copy-{}.db", std::process::id()));
        std::fs::copy(&src, &copy).unwrap();
        let _srv2 = boot(copy.to_str().unwrap(), &[]);
        let types = call("GET", "/api/task-types", None, Some(200));
        assert!(
            types
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["name"] == "summarize"),
            "{types}"
        );
        let out = Command::new(env!("CARGO_BIN_EXE_client"))
            .args(["--json", "status"])
            .env("LOOP_URL", BASE)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(serde_json::from_slice::<Value>(&out.stdout)
            .unwrap()
            .get("agents_working_now")
            .is_some());
        println!("db copy + client ok");
    }
}
