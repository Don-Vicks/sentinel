//! Channel settings over HTTP: secrets are masked on the way out, kept when a
//! masked value comes back, and bad settings are refused.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use chrono::Utc;
use sentinel::engine::Sentinel;
use sentinel::model::ChannelKind;
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

fn rule(channels: Value) -> Value {
    json!({
        "name": "Page on incidents",
        "condition": { "type": "incident", "kinds": [], "min_severity": "low" },
        "channels": channels,
    })
}

#[tokio::test]
async fn channel_secrets_are_masked_kept_and_validated() {
    let db = std::env::temp_dir().join(format!("sentinel-channels-api-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let store = Arc::new(Store::open(db.to_str().unwrap()).unwrap());
    let (bus, _) = broadcast::channel(4);
    let s = Sentinel::new(store.clone(), Arc::new(NullSource(bus)), None, PriceBook::new(), "https://sentinel.example".into()).unwrap();
    let app = sentinel::api::router(s);
    let cookie = sign_in(&app, &Keypair::new()).await;

    let channels = json!([
        { "type": "telegram", "bot_token": BOT_TOKEN, "chat_id": "-1001234567890", "min_severity": "high" },
        { "type": "pagerduty", "routing_key": ROUTING_KEY, "min_severity": "critical" },
        { "type": "slack", "url": "https://hooks.slack.com/services/T000/B000/SECRETSECRETSECRET" },
    ]);
    let (st, created, _) = call(&app, "POST", "/api/rules", Some(&cookie), Some(rule(channels))).await;
    assert_eq!(st, StatusCode::OK, "{created}");
    let id = created["id"].as_i64().unwrap();

    // Secrets never leave the server, in the create response or the list.
    let (_, listed, _) = call(&app, "GET", "/api/rules", Some(&cookie), None).await;
    for body in [created.to_string(), listed.to_string()] {
        for secret in [BOT_TOKEN, ROUTING_KEY, "SECRETSECRETSECRET", "AAEhBP0av28OdpTEST"] {
            assert!(!body.contains(secret), "{secret} leaked in {body}");
        }
    }
    let masked = &listed[0]["channels"];
    assert_eq!(masked[0]["type"], "telegram");
    assert_eq!(masked[0]["chat_id"], "-1001234567890");
    assert_eq!(masked[0]["min_severity"], "high");
    assert!(masked[0]["bot_token"].as_str().unwrap().starts_with("••••"));
    assert!(masked[2]["url"].as_str().unwrap().starts_with("https://hooks.slack.com/"));

    // The stored rule has the real values.
    let stored = store.rules().unwrap().remove(0);
    assert!(matches!(&stored.channels[0].kind, ChannelKind::Telegram { bot_token, .. } if bot_token == BOT_TOKEN));

    // Sending the masked form back (an edit that changes only the severity) keeps the secrets.
    let mut edited = masked.clone();
    edited[0]["min_severity"] = json!("medium");
    let (st, _, _) = call(&app, "PATCH", &format!("/api/rules/{id}"), Some(&cookie), Some(rule(edited))).await;
    assert_eq!(st, StatusCode::OK);
    let stored = store.rules().unwrap().remove(0);
    match (&stored.channels[0].kind, &stored.channels[1].kind, &stored.channels[2].kind) {
        (ChannelKind::Telegram { bot_token, .. }, ChannelKind::Pagerduty { routing_key }, ChannelKind::Slack { url }) => {
            assert_eq!(bot_token, BOT_TOKEN);
            assert_eq!(routing_key, ROUTING_KEY);
            assert!(url.ends_with("SECRETSECRETSECRET"));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(stored.channels[0].min_severity, Some(sentinel::model::Severity::Medium));

    // A masked secret with nothing stored to restore is refused.
    let (st, body, _) = call(&app, "POST", "/api/rules", Some(&cookie), Some(rule(masked.clone()))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");

    // Settings that can't work are refused with a reason.
    for (bad, why) in [
        (json!([{ "type": "telegram", "bot_token": "not-a-token", "chat_id": "-100123" }]), "token"),
        (json!([{ "type": "telegram", "bot_token": BOT_TOKEN, "chat_id": "somebody" }]), "chat id"),
        (json!([{ "type": "pagerduty", "routing_key": "short" }]), "routing key"),
        (json!([{ "type": "slack", "url": "http://169.254.169.254/x" }]), "private network"),
        (json!((0..6).map(|_| json!({ "type": "webhook", "url": "https://example.com/h" })).collect::<Vec<_>>()), "at most"),
    ] {
        let (st, body, _) = call(&app, "POST", "/api/rules", Some(&cookie), Some(rule(bad))).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{why}: {body}");
        assert!(body["error"].as_str().unwrap().contains(why), "{why}: {body}");
    }

    // A rule with only the old single URL still works and is masked too.
    let (st, legacy, _) = call(
        &app,
        "POST",
        "/api/rules",
        Some(&cookie),
        Some(json!({
            "name": "old style",
            "condition": { "type": "incident", "kinds": [], "min_severity": "low" },
            "webhook_url": "https://discord.com/api/webhooks/123456/AbCdEf-secret_token",
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{legacy}");
    assert!(!legacy.to_string().contains("AbCdEf"), "{legacy}");
    let rule_id = legacy["id"].as_i64().unwrap();
    let stored = store.rules().unwrap().into_iter().find(|r| r.id == rule_id).unwrap();
    assert!(matches!(&stored.targets()[0].kind, ChannelKind::Discord { .. }), "URL type is detected");
    let _ = std::fs::remove_file(&db);
}

#[tokio::test]
async fn summary_schedules_are_validated_masked_and_private() {
    let db = std::env::temp_dir().join(format!("sentinel-schedules-api-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let store = Arc::new(Store::open(db.to_str().unwrap()).unwrap());
    let (bus, _) = broadcast::channel(4);
    let s = Sentinel::new(store.clone(), Arc::new(NullSource(bus)), None, PriceBook::new(), "https://sentinel.example".into()).unwrap();
    let app = sentinel::api::router(s);
    let owner = sign_in(&app, &Keypair::new()).await;
    let other = sign_in(&app, &Keypair::new()).await;
    const PUMP: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";
    let (st, _, _) = call(&app, "POST", "/api/programs", Some(&owner), Some(json!({ "program_id": PUMP }))).await;
    assert_eq!(st, StatusCode::OK);

    let telegram = json!({ "type": "telegram", "bot_token": BOT_TOKEN, "chat_id": "-1001234567890" });
    let good = json!({ "program_id": PUMP, "period": "daily", "hour_utc": 8, "channels": [telegram] });
    let (st, created, _) = call(&app, "POST", "/api/summary-schedules", Some(&owner), Some(good.clone())).await;
    assert_eq!(st, StatusCode::OK, "{created}");
    assert!(!created.to_string().contains(BOT_TOKEN), "token is masked: {created}");
    let id = created["id"].as_i64().unwrap();

    // Not for programs you don't watch, nor with no channel, a page, or a bad hour.
    for (bad, why) in [
        (json!({ "program_id": PUMP, "period": "daily", "channels": [telegram] }), None),
        (json!({ "program_id": PUMP, "period": "daily", "hour_utc": 25, "channels": [telegram] }), Some("hour_utc")),
        (json!({ "program_id": PUMP, "period": "daily", "channels": [] }), Some("at least one")),
        (json!({ "program_id": PUMP, "period": "weekly", "channels": [{ "type": "pagerduty", "routing_key": ROUTING_KEY }] }), Some("PagerDuty")),
    ] {
        let (st, body, _) = call(&app, "POST", "/api/summary-schedules", Some(&owner), Some(bad)).await;
        match why {
            None => assert_eq!(st, StatusCode::OK, "{body}"),
            Some(why) => {
                assert_eq!(st, StatusCode::BAD_REQUEST, "{why}: {body}");
                assert!(body["error"].as_str().unwrap().contains(why), "{why}: {body}");
            }
        }
    }
    let (st, _, _) = call(&app, "POST", "/api/summary-schedules", Some(&other), Some(good.clone())).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "must watch the program first");

    // Editing with the masked token keeps the real one; other accounts can't see or touch it.
    let mut edited = json!({ "program_id": PUMP, "period": "weekly", "hour_utc": 7, "channels": created["channels"].clone() });
    edited["enabled"] = json!(false);
    let (st, updated, _) = call(&app, "PATCH", &format!("/api/summary-schedules/{id}"), Some(&owner), Some(edited)).await;
    assert_eq!(st, StatusCode::OK, "{updated}");
    let stored = store.schedules().unwrap().into_iter().find(|x| x.id == id).unwrap();
    assert!(matches!(&stored.channels[0].kind, ChannelKind::Telegram { bot_token, .. } if bot_token == BOT_TOKEN));
    assert_eq!((stored.hour_utc, stored.enabled), (7, false));
    let (_, theirs, _) = call(&app, "GET", "/api/summary-schedules", Some(&other), None).await;
    assert_eq!(theirs, json!([]));
    let (st, _, _) = call(&app, "DELETE", &format!("/api/summary-schedules/{id}"), Some(&other), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _, _) = call(&app, "DELETE", &format!("/api/summary-schedules/{id}"), Some(&owner), None).await;
    assert_eq!(st, StatusCode::OK);
    let _ = std::fs::remove_file(&db);
}

#[tokio::test]
async fn protect_this_program_creates_the_usual_rules_once() {
    let db = std::env::temp_dir().join(format!("sentinel-protect-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let store = Arc::new(Store::open(db.to_str().unwrap()).unwrap());
    let (bus, _) = broadcast::channel(4);
    let s = Sentinel::new(store.clone(), Arc::new(NullSource(bus)), None, PriceBook::new(), "https://sentinel.example".into()).unwrap();
    let app = sentinel::api::router(s);
    let cookie = sign_in(&app, &Keypair::new()).await;
    const PUMP: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";
    let path = format!("/api/programs/{PUMP}/protect");
    let telegram = json!({ "type": "telegram", "bot_token": BOT_TOKEN, "chat_id": "-1001234567890" });

    // It has to be a program you watch, and it needs somewhere to send alerts.
    let (st, _, _) = call(&app, "POST", &path, Some(&cookie), Some(json!({ "channels": [telegram] }))).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    call(&app, "POST", "/api/programs", Some(&cookie), Some(json!({ "program_id": PUMP }))).await;
    let (st, body, _) = call(&app, "POST", &path, Some(&cookie), Some(json!({}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");

    let (st, done, _) = call(&app, "POST", &path, Some(&cookie), Some(json!({ "channels": [telegram] }))).await;
    assert_eq!(st, StatusCode::OK, "{done}");
    let names: Vec<&str> = done["created"].as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert_eq!(names.len(), 5, "{names:?}");
    for want in ["any incident of high severity", "failure rate above 20%", "admin instruction from a new wallet", "health score below 60", "Sentinel feed problems"] {
        assert!(names.iter().any(|n| n.contains(want)), "{want} in {names:?}");
    }
    assert!(!done.to_string().contains(BOT_TOKEN), "secrets stay masked");
    assert!(done["next_steps"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains("vault")));
    let rules = store.rules().unwrap();
    assert_eq!(rules.len(), 5);
    assert!(rules.iter().all(|r| r.channels.len() == 1 && r.has_targets()));
    assert!(rules.iter().any(|r| matches!(r.condition, sentinel::model::Condition::Instruction { first_seen_signer: true, .. })));

    // Running it again changes nothing.
    let (_, again, _) = call(&app, "POST", &path, Some(&cookie), Some(json!({ "channels": [telegram] }))).await;
    assert_eq!(again["created"], json!([]));
    assert_eq!(again["already_had"].as_array().unwrap().len(), 5);
    assert_eq!(store.rules().unwrap().len(), 5);

    // Channels can be copied from an existing rule, so the secret never travels again.
    let first = rules[0].id;
    let other = "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4";
    call(&app, "POST", "/api/programs", Some(&cookie), Some(json!({ "program_id": other }))).await;
    let (st, copied, _) = call(&app, "POST", &format!("/api/programs/{other}/protect"), Some(&cookie), Some(json!({ "channels_from_rule": first }))).await;
    assert_eq!(st, StatusCode::OK, "{copied}");
    let new_rules: Vec<_> = store.rules().unwrap().into_iter().filter(|r| r.program_id.as_deref() == Some(other)).collect();
    assert_eq!(new_rules.len(), 4, "the feed rule already exists");
    assert!(matches!(&new_rules[0].channels[0].kind, ChannelKind::Telegram { bot_token, .. } if bot_token == BOT_TOKEN));
    let _ = std::fs::remove_file(&db);
}
