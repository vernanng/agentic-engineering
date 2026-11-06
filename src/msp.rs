//! MSP backend: one step = one turn on a private `muse serve` host.
// shortcut: fresh host + session per step; tool approvals denied; no resume; usage unreported so the bytes/4 proxy applies.
use muse_code_sdk::connection::{Connection, StdioTransport};
use muse_code_sdk::handshake::Handshake;
use muse_code_sdk::msp::{ClientInfo, InitializeParams};
use muse_code_sdk::spawn::{SpawnOptions, spawn_muse};
use serde_json::{Map, Value, json};
use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;
use std::time::Duration;

// Stay inside the 60s step budget so a stuck turn fails instead of hanging the runner.
const TURN_TIMEOUT: Duration = Duration::from_secs(50);

async fn command(
    connection: &Connection,
    method: &str,
    params: Value,
) -> Result<Map<String, Value>, String> {
    let mut map = params.as_object().cloned().unwrap_or_default();
    if !map.contains_key("commandId") {
        map.insert("commandId".into(), json!(Uuid::now_v7().to_string()));
    }
    connection
        .request(method, Some(map))
        .await
        .map_err(|e| format!("msp {method} failed: {e:?}"))
}

/// Pick the server-offered deny choice; clients never invent choice ids.
fn deny_choice(params: &Map<String, Value>) -> Option<String> {
    params
        .get("availableChoices")
        .and_then(Value::as_array)?
        .iter()
        .find(|c| {
            c.get("decision")
                .and_then(Value::as_str)
                .unwrap_or("")
                .starts_with("denied")
        })
        .and_then(|c| c.get("choiceId"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

async fn deny_approval(
    connection: &Connection,
    session_id: &str,
    params: &Map<String, Value>,
) -> Result<(), String> {
    let deny = deny_choice(params).ok_or("msp approval offers no deny choice")?;
    command(
        connection,
        "approval/decide",
        json!({
            "sessionId": session_id,
            "approvalId": params.get("approvalId").cloned().unwrap_or(Value::Null),
            "requirementId": params.get("currentRequirementId").cloned().unwrap_or(Value::Null),
            "choiceId": deny,
            "feedback": null,
        }),
    )
    .await?;
    Ok(())
}

async fn drive_turn(
    connection: &Connection,
    workspace_root: &str,
    prompt: &str,
) -> Result<(String, String), String> {
    let mut rx = connection.subscribe_notifications();
    let res = command(
        connection,
        "session/start",
        json!({ "workspaceRoot": workspace_root }),
    )
    .await?;
    let session_id = res
        .get("session")
        .and_then(|s| s.get("sessionId"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if session_id.is_empty() {
        return Err("msp session/start returned no sessionId".into());
    }
    let ack = command(
        connection,
        "turn/start",
        json!({ "sessionId": session_id, "input": [{ "type": "text", "text": prompt }] }),
    )
    .await?;
    let turn_id = ack
        .get("turnId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if turn_id.is_empty() {
        return Err("msp turn/start returned no turnId".into());
    }
    let mut text = String::new();
    loop {
        let n = match rx.recv().await {
            Ok(n) => n,
            Err(RecvError::Lagged(_)) => continue, // item/completed stays authoritative
            Err(RecvError::Closed) => return Err("msp notification stream closed".into()),
        };
        let p = n.params.as_ref();
        if p.and_then(|p| p.get("sessionId")).and_then(Value::as_str) != Some(session_id.as_str()) {
            continue;
        }
        match n.method.as_str() {
            "item/delta" => {
                let field = p
                    .and_then(|p| p.get("field"))
                    .and_then(Value::as_str)
                    .unwrap_or("text");
                if field == "text" {
                    if let Some(d) = p.and_then(|p| p.get("delta")).and_then(Value::as_str) {
                        text.push_str(d);
                    }
                }
            }
            "item/completed" => {
                if let Some(item) = p.and_then(|p| p.get("item")) {
                    if item.get("kind").and_then(Value::as_str) == Some("agentMessage") {
                        text = item
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                    }
                }
            }
            "approval/requested" => {
                if let Some(p) = p {
                    deny_approval(connection, &session_id, p).await?;
                }
            }
            "turn/completed" => {
                let ours_turn =
                    p.and_then(|p| p.get("turnId")).and_then(Value::as_str) == Some(turn_id.as_str());
                if ours_turn {
                    let terminal = p
                        .and_then(|p| p.get("terminal"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if terminal == "completed" {
                        return Ok((text, terminal));
                    }
                    let why = p
                        .and_then(|p| p.get("error"))
                        .and_then(|e| e.get("message"))
                        .and_then(Value::as_str)
                        .or_else(|| p.and_then(|p| p.get("reason")).and_then(Value::as_str))
                        .unwrap_or("");
                    return Err(format!("msp turn {terminal}: {why}"));
                }
            }
            _ => {}
        }
    }
}

pub async fn run_msp_turn(workspace_root: &str, prompt: &str) -> Result<(String, String), String> {
    let mut options = SpawnOptions::default();
    options.muse_bin = Some(
        std::env::var("MUSE_BIN")
            .unwrap_or_else(|_| "muse".into())
            .into(),
    );
    options.cwd = Some(workspace_root.into());
    let mut host = spawn_muse(options).map_err(|e| format!("msp spawn failed: {e:?}"))?;
    let transport = StdioTransport::new(
        host.take_stdin().ok_or("msp host has no stdin")?,
        host.take_stdout().ok_or("msp host has no stdout")?,
    );
    let (connection, driver) = Connection::new(transport);
    tokio::spawn(driver.run());
    // One deadline for handshake + turn: initialize has no timeout of its own.
    let deadline = tokio::time::Instant::now() + TURN_TIMEOUT;
    let initialized = match tokio::time::timeout_at(
        deadline,
        Handshake::new(connection).initialize(InitializeParams {
            client_info: ClientInfo {
                name: "loop_app".into(),
                version: "0.1.0".into(),
                title: None,
            },
            capabilities: None,
        }),
    )
    .await
    {
        // The handshake consumes the connection, so only the host is left to reap.
        Err(_) => {
            let _ = host.close().await;
            return Err(format!("msp handshake timed out after {}s", TURN_TIMEOUT.as_secs()));
        }
        Ok(Err(e)) => {
            let _ = host.close().await;
            return Err(format!("msp handshake failed: {e:?}"));
        }
        Ok(Ok(init)) => init,
    };
    let connection = initialized.connection().clone();
    let turn = drive_turn(&connection, workspace_root, prompt);
    let outcome = match tokio::time::timeout_at(deadline, turn).await {
        Ok(res) => res,
        Err(_) => Err(format!("msp turn timed out after {}s", TURN_TIMEOUT.as_secs())),
    };
    connection.close().await;
    let _ = host.close().await;
    outcome
}

#[cfg(test)]
mod msp_tests {
    use super::*;

    #[test]
    fn deny_choice_picks_denied() {
        let params: Map<String, Value> = serde_json::from_value(json!({
            "availableChoices": [
                {"choiceId": "allow_once", "decision": "approved"},
                {"choiceId": "deny", "decision": "denied"}
            ]
        }))
        .unwrap();
        assert_eq!(deny_choice(&params).as_deref(), Some("deny"));
        let params: Map<String, Value> = serde_json::from_value(json!({
            "availableChoices": [{"choiceId": "allow_once", "decision": "approved"}]
        }))
        .unwrap();
        assert_eq!(deny_choice(&params), None);
    }

    #[tokio::test(flavor = "current_thread")]
    #[ignore] // live: needs a muse login and spends budget; run explicitly
    async fn live_deny_blocks_write() {
        let probe = std::env::temp_dir().join("loop-msp-deny-probe.txt");
        let _ = std::fs::remove_file(&probe);
        let ws = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let prompt = format!(
            "Use a file-writing tool to write the single line 'breach' to {}, then reply with exactly this JSON and nothing else: {{\"output\": \"deny-probe\", \"confidence\": 1.0}}",
            probe.display()
        );
        let (text, terminal) = run_msp_turn(&ws, &prompt).await.unwrap();
        assert_eq!(terminal, "completed");
        assert!(text.contains("deny-probe"), "unexpected turn text: {text}");
        assert!(!probe.exists(), "denied write landed on disk");
        let _ = std::fs::remove_file(&probe);
    }

    #[tokio::test(flavor = "current_thread")]
    #[ignore] // live: needs a muse login and spends budget; run explicitly
    async fn live_turn_smoke() {
        let ws = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let (text, terminal) = run_msp_turn(&ws, "Reply with exactly this JSON and nothing else: {\"output\": \"smoke\", \"confidence\": 1.0}").await.unwrap();
        assert_eq!(terminal, "completed");
        assert!(text.contains("smoke"), "unexpected turn text: {text}");
    }
}
