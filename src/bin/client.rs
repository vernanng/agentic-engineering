//! Detached CLI client. Usage: client [--json] status|feed|runs|send <id> "..."|vote <id> up|down
//! Env: LOOP_URL (default http://localhost:8000), LOOP_TOKEN.
use serde_json::{json, Value};

fn call(method: &str, base: &str, token: &str, path: &str, data: Option<Value>) -> Value {
    let url = format!("{base}{path}");
    let mut req = match method {
        "GET" => ureq::get(&url),
        _ => ureq::post(&url),
    };
    if !token.is_empty() {
        req = req.set("X-Loop-Token", token);
    }
    let resp = match data {
        Some(d) => req.send_json(d),
        None => req.call(),
    };
    match resp {
        Ok(r) => r.into_json().unwrap_or(json!({})),
        Err(ureq::Error::Status(c, r)) => {
            eprintln!(
                "error {c}: {}",
                r.into_string()
                    .unwrap_or_default()
                    .chars()
                    .take(200)
                    .collect::<String>()
            );
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

fn main() {
    let base = std::env::var("LOOP_URL").unwrap_or_else(|_| "http://localhost:8000".into());
    let token = std::env::var("LOOP_TOKEN").unwrap_or_default();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let as_json = argv.iter().any(|a| a == "--json");
    let args: Vec<String> = argv.into_iter().filter(|a| a != "--json").collect();
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("status");
    let out = match (cmd, args.len()) {
        ("status", _) => call("GET", &base, &token, "/api/executions", None),
        ("feed", _) => call("GET", &base, &token, "/api/items?status=presented", None),
        ("runs", _) => call("GET", &base, &token, "/api/runs", None),
        ("send", 3) => call(
            "POST",
            &base,
            &token,
            "/api/runs",
            Some(json!({"task_type_id": args[1].parse::<i64>().unwrap_or(-1), "input": args[2]})),
        ),
        ("vote", 3) => call(
            "POST",
            &base,
            &token,
            &format!("/api/items/{}/vote", args[1]),
            Some(json!({"vote": args[2]})),
        ),
        _ => {
            eprintln!(
                "usage: client [--json] status|feed|runs|send <id> \"...\"|vote <id> up|down"
            );
            std::process::exit(2);
        }
    };
    if as_json || !(out.is_object() || out.is_array()) {
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
        return;
    }
    match cmd {
        "status" => {
            println!(
                "agents working now: {} (live runs {} + enabled crons {})",
                out["agents_working_now"], out["live_runs"], out["enabled_crons"]
            );
            println!(
                "jev calls: {} (mock {}) cost ${:.5} · step tokens {}",
                out["jev_calls"],
                out["jev_mock_calls"],
                out["jev_cost"].as_f64().unwrap_or(0.0),
                out["step_tokens"]
            );
            println!("runs: {}", out["runs_by_status"]);
        }
        "feed" => {
            for i in out.as_array().unwrap() {
                println!(
                    "#{} [H{} F{} S{}] {}",
                    i["id"],
                    i["hype"],
                    i["facts"],
                    i["signal"],
                    i["title"].as_str().unwrap_or("")
                );
            }
        }
        "runs" => {
            for r in out.as_array().unwrap() {
                println!(
                    "#{} task={} mode={} {} {}",
                    r["id"],
                    r["task_type_id"],
                    r["mode"].as_str().unwrap_or(""),
                    r["status"].as_str().unwrap_or(""),
                    r["input"]
                        .as_str()
                        .unwrap_or("")
                        .chars()
                        .take(60)
                        .collect::<String>()
                );
            }
        }
        _ => println!("{}", serde_json::to_string_pretty(&out).unwrap()),
    }
}
