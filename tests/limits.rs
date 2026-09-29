//! Abuse limits for a public instance: per-account caps, operator-only
//! settings, incident permissions, rate limits and sign-in challenge caps.
//! Configured through env vars, so this file runs in its own process.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use chrono::Utc;
use sentinel::engine::Sentinel;
use sentinel::model::*;
use sentinel::pricing::PriceBook;
use sentinel::source::VortexSource;
use sentinel::store::Store;
use serde_json::{json, Value};
use solana_sdk::signature::{keypair_from_seed, Keypair, Signer};
use std::sync::{Arc, Once};
use tokio::sync::broadcast;
use tower::ServiceExt;
use vortex::events::VortexTransaction;
use vortex::hub::HubStats;

const PROGRAMS: [&str; 3] = [
    "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P",
    "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4",
    "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc",
];

fn admin() -> Keypair {
    keypair_from_seed(&[7u8; 32]).unwrap()
}

static ENV: Once = Once::new();
fn configure() {
    ENV.call_once(|| {
        std::env::set_var("SENTINEL_MAX_WATCHED", "2");
        std::env::set_var("SENTINEL_MAX_RULES", "2");
        std::env::set_var("SENTINEL_AUTH_PER_MIN", "6");
        std::env::set_var("SENTINEL_ADMINS", admin().pubkey().to_string());
    });
}

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

fn app(name: &str) -> (Router, Arc<Sentinel>) {
    configure();
    let db = std::env::temp_dir().join(format!("sentinel-limits-{name}-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let store = Arc::new(Store::open(db.to_str().unwrap()).unwrap());
    let (bus, _) = broadcast::channel(4);
    let s = Sentinel::new(store, Arc::new(NullSource(bus)), None, PriceBook::new(), "http://localhost:8080".into()).unwrap();
    (sentinel::api::router(s.clone()), s)
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
    let (_, ch, _) = call(app, "POST", "/api/auth/challenge", None, Some(json!({ "pubkey": pubkey }))).await;
    let message = ch["message"].as_str().unwrap().to_string();
    let signature = kp.sign_message(message.as_bytes()).to_string();
    let (st, body, cookie) = call(app, "POST", "/api/auth/verify", None, Some(json!({ "pubkey": pubkey, "message": message, "signature": signature }))).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    cookie.unwrap()
}

fn rule(n: usize) -> Value {
    json!({ "name": format!("r{n}"), "condition": { "type": "incident", "kinds": [], "min_severity": "low" } })
}

#[tokio::test]
async fn per_account_caps() {
    let (app, _) = app("caps");
    let user = sign_in(&app, &Keypair::new()).await;
    for p in &PROGRAMS[..2] {
        let (st, _, _) = call(&app, "POST", "/api/programs", Some(&user), Some(json!({ "program_id": p }))).await;
        assert_eq!(st, StatusCode::OK);
    }
    let (st, body, _) = call(&app, "POST", "/api/programs", Some(&user), Some(json!({ "program_id": PROGRAMS[2] }))).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert!(body["error"].as_str().unwrap().contains("watchlist is full"));
    // Re-watching something already on the list isn't blocked by the cap.
    let (st, _, _) = call(&app, "POST", "/api/programs", Some(&user), Some(json!({ "program_id": PROGRAMS[0] }))).await;
    assert_eq!(st, StatusCode::OK);

    for n in 0..2 {
        let (st, _, _) = call(&app, "POST", "/api/rules", Some(&user), Some(rule(n))).await;
        assert_eq!(st, StatusCode::OK);
    }
    let (st, _, _) = call(&app, "POST", "/api/rules", Some(&user), Some(rule(3))).await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // The operator isn't capped.
    let op = sign_in(&app, &admin()).await;
    for p in PROGRAMS {
        let (st, _, _) = call(&app, "POST", "/api/programs", Some(&op), Some(json!({ "program_id": p }))).await;
        assert_eq!(st, StatusCode::OK);
    }
}

#[tokio::test]
async fn shared_settings_and_incidents_need_the_right_account() {
    let (app, s) = app("perms");
    let (watcher, stranger, op) = (Keypair::new(), Keypair::new(), admin());
    let cw = sign_in(&app, &watcher).await;
    let cs = sign_in(&app, &stranger).await;
    let co = sign_in(&app, &op).await;
    call(&app, "POST", "/api/programs", Some(&cw), Some(json!({ "program_id": PROGRAMS[0] }))).await;

    // Detection settings are shared: operator only.
    let patch = json!({ "detection": { "failure_multiplier": 4.0 } });
    let path = format!("/api/programs/{}", PROGRAMS[0]);
    let (st, _, _) = call(&app, "PATCH", &path, Some(&cw), Some(patch.clone())).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, body, _) = call(&app, "PATCH", &path, Some(&co), Some(patch)).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["detection"]["failure_multiplier"], 4.0);

    // Only watchers (or the operator) change an incident's status.
    let inc = s
        .store
        .create_incident(Incident {
            id: 0,
            program_id: PROGRAMS[0].into(),
            kind: IncidentKind::FailureSpike,
            severity: Severity::High,
            status: IncidentStatus::Open,
            title: "t".into(),
            summary: "s".into(),
            explanation: "e".into(),
            source: "detector".into(),
            metric: None,
            observed: None,
            peak: None,
            baseline: None,
            threshold: None,
            onset_at: None,
            detected_at: Utc::now(),
            updated_at: Utc::now(),
            resolved_at: None,
            detection_latency_ms: None,
            affected_count: 0,
            affected_wallets: 0,
            evidence: json!({}),
        })
        .unwrap();
    let ipath = format!("/api/incidents/{}", inc.id);
    let (st, _, _) = call(&app, "PATCH", &ipath, Some(&cs), Some(json!({ "status": "resolved" }))).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _, _) = call(&app, "PATCH", &ipath, Some(&cw), Some(json!({ "status": "investigating" }))).await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
async fn sign_in_is_rate_limited_and_challenges_are_capped() {
    let (app, s) = app("rate");
    let pk = Keypair::new();
    let body = json!({ "pubkey": pk.pubkey().to_string() });
    for _ in 0..6 {
        let (st, _, _) = call(&app, "POST", "/api/auth/challenge", None, Some(body.clone())).await;
        assert_eq!(st, StatusCode::OK);
    }
    let (st, _, _) = call(&app, "POST", "/api/auth/challenge", None, Some(body)).await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS);
    // Reads aren't limited.
    for _ in 0..20 {
        let (st, _, _) = call(&app, "GET", "/api/status", None, None).await;
        assert_eq!(st, StatusCode::OK);
    }

    // Only the newest few challenges per address stay valid.
    let kp = Keypair::new();
    let addr = kp.pubkey().to_string();
    let first = s.auth.challenge(&addr).unwrap();
    for _ in 0..3 {
        s.auth.challenge(&addr).unwrap();
    }
    let sig = kp.sign_message(first.as_bytes()).to_string();
    assert!(s.auth.verify(&addr, &first, &sig).is_err(), "superseded challenge is gone");
    let latest = s.auth.challenge(&addr).unwrap();
    let sig = kp.sign_message(latest.as_bytes()).to_string();
    assert!(s.auth.verify(&addr, &latest, &sig).is_ok());
}
