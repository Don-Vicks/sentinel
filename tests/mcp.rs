//! The MCP server over HTTP: tokens, scopes, tools, resources and prompts.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use chrono::Utc;
use sentinel::engine::Sentinel;
use sentinel::pricing::PriceBook;
use sentinel::source::VortexSource;
use sentinel::store::Store;
use serde_json::{json, Value};
use solana_sdk::signature::{Keypair, Signer};
use std::sync::Arc;
use tokio::sync::broadcast;
use tower::ServiceExt;
use vortex::events::VortexTransaction;
use vortex::hub::HubStats;

const BOT_TOKEN: &str = "123456789:AAEhBP0av28OdpTEST-token_value";
const ROUTING_KEY: &str = "R0UT1NGKEYFORTESTSONLY0123456789";

struct NullSource(broadcast::Sender<Arc<VortexTransaction>>);
impl VortexSource for NullSource {
    fn subscribe(&self) -> broadcast::Receiver<Arc<VortexTransaction>> {
        self.0.subscribe()
    }
    fn watch_programs(&self, _: Vec<String>) {}
    fn health(&self) -> HubStats {
        HubStats { transactions: 0, last_slot: 0, last_transaction_at: None, started_at: Utc::now(), programs: vec![], subscribers: 0 }
    }
}

async fn call(app: &Router, method: &str, path: &str, cookie: Option<&str>, body: Option<Value>) -> (StatusCode, Value, Option<String>) {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(c) = cookie {
        req = req.header(header::COOKIE, c);
    }
    let req = match body {
        Some(b) => req.header(header::CONTENT_TYPE, "application/json").body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let set_cookie = res
        .headers()
        .get(header::SET_COOKIE)
        .map(|v| v.to_str().unwrap().split(';').next().unwrap().to_string());
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null), set_cookie)
}

async fn sign_in(app: &Router, kp: &Keypair) -> String {
    let pubkey = kp.pubkey().to_string();
    let (_, body, _) = call(app, "POST", "/api/auth/challenge", None, Some(json!({ "pubkey": pubkey }))).await;
    let message = body["message"].as_str().unwrap().to_string();
    let signature = kp.sign_message(message.as_bytes()).to_string();
    let (st, body, cookie) = call(app, "POST", "/api/auth/verify", None, Some(json!({ "pubkey": pubkey, "message": message, "signature": signature }))).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    cookie.expect("session cookie")
}


async fn rpc(app: &Router, token: Option<&str>, body: Value) -> (StatusCode, Value) {
    let mut req = Request::builder().method("POST").uri("/mcp").header(header::CONTENT_TYPE, "application/json");
    if let Some(t) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let res = app.clone().oneshot(req.body(Body::from(body.to_string())).unwrap()).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 22).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn tool(id: i64, tool: &str, args: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": tool, "arguments": args } })
}

/// The structured result of a tool call that is expected to succeed.
fn ok(reply: &Value) -> Value {
    assert!(reply["error"].is_null(), "{reply}");
    assert!(reply["result"]["isError"].is_null(), "tool failed: {reply}");
    reply["result"]["structuredContent"].clone()
}

fn failed(reply: &Value) -> String {
    assert_eq!(reply["result"]["isError"], true, "{reply}");
    reply["result"]["content"][0]["text"].as_str().unwrap().to_string()
}

const PUMP: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";

#[tokio::test]
async fn agents_can_read_and_act_within_their_token_scope() {
    let db = std::env::temp_dir().join(format!("sentinel-mcp-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let store = Arc::new(Store::open(db.to_str().unwrap()).unwrap());
    let (bus, _) = broadcast::channel(4);
    let s = Sentinel::new(store.clone(), Arc::new(NullSource(bus)), None, PriceBook::new(), "https://sentinel.example".into()).unwrap();
    s.add_program(PUMP.into(), None).unwrap();
    let app = sentinel::api::router(s.clone());
    let cookie = sign_in(&app, &Keypair::new()).await;
    let other_cookie = sign_in(&app, &Keypair::new()).await;

    // Tokens are created from a signed-in session; the secret is shown once.
    let (st, read, _) = call(&app, "POST", "/api/tokens", Some(&cookie), Some(json!({ "name": "claude", "scope": "read" }))).await;
    assert_eq!(st, StatusCode::OK, "{read}");
    let read_token = read["secret"].as_str().unwrap().to_string();
    assert!(read_token.starts_with("snt_"));
    let (_, write, _) = call(&app, "POST", "/api/tokens", Some(&cookie), Some(json!({ "name": "ops", "scope": "write" }))).await;
    let write_token = write["secret"].as_str().unwrap().to_string();
    let (_, listed, _) = call(&app, "GET", "/api/tokens", Some(&cookie), None).await;
    assert_eq!(listed.as_array().unwrap().len(), 2);
    assert!(!listed.to_string().contains(&read_token), "the secret is not listed again");
    let (st, _, _) = call(&app, "POST", "/api/tokens", Some(&cookie), Some(json!({ "name": "x", "scope": "admin" }))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // No token, or a wrong one, is refused.
    let ping = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" });
    assert_eq!(rpc(&app, None, ping.clone()).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(rpc(&app, Some("snt_nope"), ping.clone()).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(rpc(&app, Some(&cookie), ping.clone()).await.0, StatusCode::UNAUTHORIZED, "a session cookie is not a token");

    // The handshake.
    let (st, init) = rpc(&app, Some(&read_token), json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": { "name": "t", "version": "1" } } })).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(init["result"]["serverInfo"]["name"], "vortex-sentinel");
    assert!(init["result"]["capabilities"]["tools"].is_object());
    let (st, _) = rpc(&app, Some(&read_token), json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).await;
    assert_eq!(st, StatusCode::ACCEPTED, "notifications get no answer");
    let (_, bad) = rpc(&app, Some(&read_token), json!({ "jsonrpc": "2.0", "id": 2, "method": "nope" })).await;
    assert_eq!(bad["error"]["code"], -32601);

    // Tools: every one has a schema, and write tools say they need a write token.
    let (_, list) = rpc(&app, Some(&read_token), json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" })).await;
    let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    for want in ["list_programs", "get_health", "get_summary", "list_incidents", "diagnose_incident", "get_incident_report", "explain_transaction", "get_posture", "create_rule", "watch_program", "set_vaults"] {
        assert!(names.contains(&want), "{want} missing from {names:?}");
    }
    let create = list["result"]["tools"].as_array().unwrap().iter().find(|t| t["name"] == "create_rule").unwrap();
    assert!(create["description"].as_str().unwrap().contains("needs a write-scoped token"));
    assert_eq!(create["annotations"]["readOnlyHint"], false);
    assert!(list["result"]["tools"].as_array().unwrap().iter().all(|t| t["inputSchema"]["type"] == "object"));

    // Reading.
    let programs = ok(&rpc(&app, Some(&read_token), tool(4, "list_programs", json!({}))).await.1);
    assert_eq!(programs["programs"][0]["program_id"], PUMP);
    assert_eq!(programs["programs"][0]["label"], "Pump.fun");
    // A program can be named by its label.
    let health = ok(&rpc(&app, Some(&read_token), tool(5, "get_health", json!({ "program": "pump.fun" }))).await.1);
    assert_eq!(health["status"], "learning");
    assert!(health["checks"].as_array().unwrap().iter().any(|c| c["id"] == "reliability"));
    let summary = ok(&rpc(&app, Some(&read_token), tool(6, "get_summary", json!({ "program": PUMP, "period": "7d" }))).await.1);
    assert_eq!(summary["period_secs"], 7 * 86_400);
    assert!(failed(&rpc(&app, Some(&read_token), tool(7, "get_health", json!({ "program": "Nowhere" }))).await.1).contains("No monitored program"));
    assert!(failed(&rpc(&app, Some(&read_token), tool(8, "get_summary", json!({ "program": PUMP, "period": "soon" }))).await.1).contains("period"));

    // An incident, diagnosed.
    let incident = store
        .create_incident(
            serde_json::from_value(json!({
                "id": 0, "program_id": PUMP, "kind": "failure_spike", "severity": "high", "status": "open",
                "title": "Transaction failure spike · Pump.fun", "summary": "45% of transactions failing",
                "explanation": "Failure rate rose.", "source": "detector", "metric": null, "observed": null, "peak": null,
                "baseline": null, "threshold": null, "onset_at": null, "detected_at": "2024-01-01T00:00:00Z",
                "updated_at": "2024-01-01T00:00:00Z", "resolved_at": null, "detection_latency_ms": 3000,
                "affected_count": 40, "affected_wallets": 12,
                "evidence": { "fingerprints": [{ "key": "k", "program_id": PUMP, "program_name": "Pump.fun", "instruction": "Sell", "error": "TooLittleSolReceived", "code": 6003, "count": 38, "share": 0.95, "samples": [] }],
                              "deploy": { "note": "This began 74s after the program was upgraded (transaction abcd…, incident #1000)." } }
            }))
            .unwrap(),
        )
        .unwrap();
    let open = ok(&rpc(&app, Some(&read_token), tool(9, "list_incidents", json!({}))).await.1);
    assert_eq!(open["incidents"][0]["id"], incident.id);
    assert_eq!(open["incidents"][0]["after_upgrade"], true);
    let diagnosis = ok(&rpc(&app, Some(&read_token), tool(10, "diagnose_incident", json!({ "id": incident.id }))).await.1);
    assert_eq!(diagnosis["confidence"], "high");
    assert!(diagnosis["likely_cause"].as_str().unwrap().contains("upgrade"));
    let (_, report) = rpc(&app, Some(&read_token), tool(11, "get_incident_report", json!({ "id": incident.id }))).await;
    let md = report["result"]["content"][0]["text"].as_str().unwrap();
    assert!(md.starts_with("# Transaction failure spike") && md.contains("## Next steps"), "{md}");
    assert!(failed(&rpc(&app, Some(&read_token), tool(12, "get_incident", json!({ "id": 999_999 }))).await.1).contains("not found"));

    // Resources and prompts.
    let (_, res) = rpc(&app, Some(&read_token), json!({ "jsonrpc": "2.0", "id": 13, "method": "resources/list" })).await;
    let uris: Vec<&str> = res["result"]["resources"].as_array().unwrap().iter().map(|r| r["uri"].as_str().unwrap()).collect();
    assert!(uris.contains(&format!("sentinel://incident/{}/report", incident.id).as_str()), "{uris:?}");
    let (_, read) = rpc(&app, Some(&read_token), json!({ "jsonrpc": "2.0", "id": 14, "method": "resources/read", "params": { "uri": format!("sentinel://incident/{}/diagnosis", incident.id) } })).await;
    assert!(read["result"]["contents"][0]["text"].as_str().unwrap().contains("upgrade"));
    let (_, prompt) = rpc(&app, Some(&read_token), json!({ "jsonrpc": "2.0", "id": 15, "method": "prompts/get", "params": { "name": "triage-incident", "arguments": { "incident_id": incident.id.to_string() } } })).await;
    let text = prompt["result"]["messages"][0]["content"]["text"].as_str().unwrap();
    assert!(text.contains("diagnose_incident") && text.contains(&format!("#{}", incident.id)), "{text}");

    // Writing needs a write-scoped token.
    let denied = failed(&rpc(&app, Some(&read_token), tool(16, "watch_program", json!({ "program_id": "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4" }))).await.1);
    assert!(denied.contains("read-only"), "{denied}");
    let watched = ok(&rpc(&app, Some(&write_token), tool(17, "watch_program", json!({ "program_id": PUMP }))).await.1);
    assert_eq!(watched["program_id"], PUMP);

    // A rule reusing another rule's channels never passes their secrets through the agent.
    let (_, base, _) = call(&app, "POST", "/api/rules", Some(&cookie), Some(json!({
        "name": "Pages", "condition": { "type": "incident", "kinds": [], "min_severity": "low" },
        "channels": [{ "type": "telegram", "bot_token": BOT_TOKEN, "chat_id": "-100123" }],
    }))).await;
    let created = ok(&rpc(&app, Some(&write_token), tool(18, "create_rule", json!({
        "name": "Failure rate above 20%", "program": "Pump.fun", "severity": "high",
        "condition": { "type": "metric", "metric": "failure_rate", "op": ">", "value": 20, "window_secs": 60 },
        "channels_from_rule": base["id"],
    }))).await.1);
    assert_eq!(created["program_id"], PUMP);
    assert_eq!(created["channels"][0]["type"], "telegram");
    assert!(!created.to_string().contains(BOT_TOKEN), "{created}");
    let stored = store.rules().unwrap().into_iter().find(|r| r.id == created["id"].as_i64().unwrap()).unwrap();
    assert!(matches!(&stored.channels[0].kind, sentinel::model::ChannelKind::Telegram { bot_token, .. } if bot_token == BOT_TOKEN));
    // Another account's token cannot borrow them.
    let (_, other) = call(&app, "POST", "/api/tokens", Some(&other_cookie), Some(json!({ "name": "theirs", "scope": "write" }))).await.1.get("secret").cloned().map(|v| ((), v)).unwrap();
    let theirs = other.as_str().unwrap().to_string();
    assert!(failed(&rpc(&app, Some(&theirs), tool(19, "create_rule", json!({
        "name": "steal", "condition": { "type": "incident", "kinds": [], "min_severity": "low" }, "channels_from_rule": base["id"],
    }))).await.1).contains("no such rule"));
    let theirs_rules = ok(&rpc(&app, Some(&theirs), tool(20, "list_rules", json!({}))).await.1);
    assert_eq!(theirs_rules["rules"], json!([]), "rules are private to their owner");

    // Revoking a token cuts access at once.
    let id = write["token"]["id"].as_i64().unwrap();
    let (st, _, _) = call(&app, "DELETE", &format!("/api/tokens/{id}"), Some(&cookie), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(rpc(&app, Some(&write_token), ping).await.0, StatusCode::UNAUTHORIZED);
    let _ = std::fs::remove_file(&db);
}
