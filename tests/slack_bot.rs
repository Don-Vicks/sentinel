//! Slack through a bot token threads an incident's updates under its first
//! message, explains Slack's refusals, and saved destinations can be tested and
//! reused by rules without ever leaking the token.

use axum::body::Body;
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::{routing::post, Json, Router};
use chrono::Utc;
use sentinel::alerts::{Alert, AlertEvent, Dispatcher};
use sentinel::engine::Sentinel;
use sentinel::model::*;
use sentinel::pricing::PriceBook;
use sentinel::source::VortexSource;
use sentinel::store::Store;
use serde_json::{json, Value};
use solana_sdk::signature::{Keypair, Signer};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast;
use tower::ServiceExt;
use vortex::events::VortexTransaction;
use vortex::hub::HubStats;

/// The Slack API address is read from the environment when a dispatcher is built.
static ENV: Mutex<()> = Mutex::new(());

const TOKEN: &str = "xoxb-1234567890-TESTONLYtokenvalue";

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
    let cookie = res.headers().get(header::SET_COOKIE).map(|v| v.to_str().unwrap().split(';').next().unwrap().to_string());
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null), cookie)
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

/// (authorization header, body) of every chat.postMessage the fake Slack saw.
type Seen = Arc<Mutex<Vec<(String, Value)>>>;

async fn fake_slack() -> (String, Seen) {
    let seen: Seen = Arc::default();
    let sink = seen.clone();
    let ts = Arc::new(AtomicU64::new(100));
    let app = Router::new().route(
        "/chat.postMessage",
        post(move |headers: HeaderMap, Json(v): Json<Value>| {
            let (sink, ts) = (sink.clone(), ts.clone());
            async move {
                let auth = headers.get("authorization").and_then(|h| h.to_str().ok()).unwrap_or_default().to_string();
                let channel = v["channel"].as_str().unwrap_or_default().to_string();
                sink.lock().unwrap().push((auth, v));
                // Slack answers 200 even when it refuses a message.
                if channel == "#nobody" {
                    return Json(json!({ "ok": false, "error": "not_in_channel" }));
                }
                Json(json!({ "ok": true, "ts": format!("1700000000.{:06}", ts.fetch_add(1, Ordering::SeqCst)) }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), seen)
}

fn new_store(tag: &str) -> Arc<Store> {
    let db = std::env::temp_dir().join(format!("sentinel-slackbot-{tag}-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db);
    Arc::new(Store::open(db.to_str().unwrap()).unwrap())
}

#[tokio::test]
async fn an_incident_stays_in_one_slack_thread() {
    let (base, seen) = fake_slack().await;
    let store = new_store("thread");
    let (live, _) = broadcast::channel(16);
    let dispatcher = {
        let _env = ENV.lock().unwrap();
        std::env::set_var("SENTINEL_ALLOW_PRIVATE_WEBHOOKS", "1");
        std::env::set_var("SENTINEL_SLACK_API", &base);
        Dispatcher::new(store.clone(), live)
    };
    let rule = AlertRule {
        id: 1,
        owner: None,
        name: "Page on incidents".into(),
        program_id: None,
        condition: Condition::Incident { kinds: vec![], min_severity: Severity::Low },
        create_incident: false,
        severity: Severity::High,
        webhook_url: None,
        channels: vec![Channel { kind: ChannelKind::SlackBot { bot_token: TOKEN.into(), channel: "#alerts".into() }, min_severity: None }],
        enabled: true,
        cooldown_secs: 0,
        created_at: Utc::now(),
        last_fired_at: None,
    };
    let alert = |event| Alert {
        rule: rule.clone(),
        program_id: "P".into(),
        program_label: "Jupiter".into(),
        message: "Failure rate 80%".into(),
        incident_id: Some(7),
        severity: Severity::High,
        event,
        payload: json!({}),
    };
    dispatcher.dispatch(alert(AlertEvent::Opened));
    for _ in 0..100 {
        if !seen.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    dispatcher.dispatch(alert(AlertEvent::Resolved));
    for _ in 0..100 {
        if seen.lock().unwrap().len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert_eq!(seen[0].0, format!("Bearer {TOKEN}"));
    assert_eq!(seen[0].1["channel"], "#alerts");
    assert!(seen[0].1.get("thread_ts").is_none(), "the first message starts the thread");
    assert_eq!(seen[1].1["thread_ts"], "1700000000.000100", "the resolution replies to the first message");
}

#[tokio::test]
async fn saved_destinations_are_tested_reused_and_private() {
    let (base, seen) = fake_slack().await;
    let store = new_store("dest");
    let (bus, _) = broadcast::channel(4);
    let s = {
        let _env = ENV.lock().unwrap();
        std::env::set_var("SENTINEL_ALLOW_PRIVATE_WEBHOOKS", "1");
        std::env::set_var("SENTINEL_SLACK_API", &base);
        Sentinel::new(store.clone(), Arc::new(NullSource(bus)), None, PriceBook::new(), "https://sentinel.example".into()).unwrap()
    };
    let app = sentinel::api::router(s);
    let me = sign_in(&app, &Keypair::new()).await;
    let other = sign_in(&app, &Keypair::new()).await;

    // A bad token or missing channel is refused with the fix.
    let (st, err, _) = call(&app, "POST", "/api/destinations", Some(&me), Some(json!({ "name": "x", "type": "slack_bot", "bot_token": "nope", "channel": "#a" }))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(err["error"].as_str().unwrap_or(&err.to_string()).contains("xoxb-"), "{err}");

    // Saving returns the destination with the token masked.
    let (st, d, _) = call(&app, "POST", "/api/destinations", Some(&me), Some(json!({ "name": "Ops on Slack", "type": "slack_bot", "bot_token": TOKEN, "channel": "#alerts" }))).await;
    assert_eq!(st, StatusCode::OK, "{d}");
    assert!(!d.to_string().contains("TESTONLYtoken"), "{d}");
    let id = d["id"].as_i64().unwrap();

    // Testing it delivers a real message and says so.
    let (st, res, _) = call(&app, "POST", &format!("/api/destinations/{id}/test"), Some(&me), None).await;
    assert_eq!(st, StatusCode::OK, "{res}");
    assert_eq!(res["delivered"], true, "{res}");
    assert_eq!(seen.lock().unwrap().last().unwrap().0, format!("Bearer {TOKEN}"));

    // A refusal from Slack comes back readable instead of as a silent success.
    let (_, res, _) = call(&app, "POST", "/api/channels/test", Some(&me), Some(json!({ "type": "slack_bot", "bot_token": TOKEN, "channel": "#nobody" }))).await;
    assert_eq!(res["delivered"], false, "{res}");
    assert!(res["error"].as_str().unwrap().contains("/invite"), "{res}");

    // A rule can use the saved destination without the token ever passing through the client.
    let (st, rule, _) = call(
        &app,
        "POST",
        "/api/rules",
        Some(&me),
        Some(json!({ "name": "Incidents", "condition": { "type": "incident", "kinds": [], "min_severity": "low" }, "destination_ids": [id] })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{rule}");
    assert_eq!(rule["channels"][0]["type"], "slack_bot");
    assert!(!rule.to_string().contains("TESTONLYtoken"), "{rule}");
    let stored = store.rules().unwrap().remove(0);
    assert!(matches!(&stored.channels[0].kind, ChannelKind::SlackBot { bot_token, .. } if bot_token == TOKEN));

    // Destinations belong to their account.
    let (_, theirs, _) = call(&app, "GET", "/api/destinations", Some(&other), None).await;
    assert_eq!(theirs, json!([]));
    let (st, _, _) = call(&app, "POST", &format!("/api/destinations/{id}/test"), Some(&other), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _, _) = call(&app, "DELETE", &format!("/api/destinations/{id}"), Some(&other), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _, _) = call(&app, "DELETE", &format!("/api/destinations/{id}"), Some(&me), None).await;
    assert_eq!(st, StatusCode::OK);
}
