//! Sign-In With Solana and per-account ownership, exercised through the real
//! HTTP router.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use chrono::Utc;
use sentinel::alerts::check_webhook_url;
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

const PUMP: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";
const JUP: &str = "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4";

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
    let db = std::env::temp_dir().join(format!("sentinel-auth-{name}-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let store = Arc::new(Store::open(db.to_str().unwrap()).unwrap());
    let (bus, _) = broadcast::channel(4);
    let s = Sentinel::new(store, Arc::new(NullSource(bus)), None, PriceBook::new(), "https://sentinel.example".into()).unwrap();
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
    let set_cookie = res
        .headers()
        .get(header::SET_COOKIE)
        .map(|v| v.to_str().unwrap().split(';').next().unwrap().to_string());
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null), set_cookie)
}

/// Full wallet sign-in; returns the session cookie.
async fn sign_in(app: &Router, kp: &Keypair) -> String {
    let pubkey = kp.pubkey().to_string();
    let (st, body, _) = call(app, "POST", "/api/auth/challenge", None, Some(json!({ "pubkey": pubkey }))).await;
    assert_eq!(st, StatusCode::OK);
    let message = body["message"].as_str().unwrap().to_string();
    let signature = kp.sign_message(message.as_bytes()).to_string();
    let (st, body, cookie) = call(
        app,
        "POST",
        "/api/auth/verify",
        None,
        Some(json!({ "pubkey": pubkey, "message": message, "signature": signature })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["account"], pubkey);
    cookie.expect("session cookie")
}

#[tokio::test]
async fn sign_in_with_solana() {
    let (app, _) = app("siws");
    let kp = Keypair::new();
    let cookie = sign_in(&app, &kp).await;
    let (_, me, _) = call(&app, "GET", "/api/auth/me", Some(&cookie), None).await;
    assert_eq!(me["account"], kp.pubkey().to_string());

    // Message format carries the domain, nonce and expiry.
    let (_, ch, _) = call(&app, "POST", "/api/auth/challenge", None, Some(json!({ "pubkey": kp.pubkey().to_string() }))).await;
    let msg = ch["message"].as_str().unwrap();
    assert!(msg.starts_with("sentinel.example wants you to sign in with your Solana account:"));
    assert!(msg.contains("Nonce: ") && msg.contains("Expiration Time: "));

    // Signed by a different wallet: rejected.
    let other = Keypair::new();
    let sig = other.sign_message(msg.as_bytes()).to_string();
    let (st, _, _) = call(&app, "POST", "/api/auth/verify", None, Some(json!({ "pubkey": kp.pubkey().to_string(), "message": msg, "signature": sig }))).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // The nonce was consumed by that attempt: even a valid signature can't reuse it.
    let good = kp.sign_message(msg.as_bytes()).to_string();
    let (st, body, _) = call(&app, "POST", "/api/auth/verify", None, Some(json!({ "pubkey": kp.pubkey().to_string(), "message": msg, "signature": good }))).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "{body}");

    // Tampered message: rejected.
    let (_, ch, _) = call(&app, "POST", "/api/auth/challenge", None, Some(json!({ "pubkey": kp.pubkey().to_string() }))).await;
    let tampered = ch["message"].as_str().unwrap().replace("Vortex Sentinel", "Evil App");
    let sig = kp.sign_message(tampered.as_bytes()).to_string();
    let (st, _, _) = call(&app, "POST", "/api/auth/verify", None, Some(json!({ "pubkey": kp.pubkey().to_string(), "message": tampered, "signature": sig }))).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // Logout ends the session.
    call(&app, "POST", "/api/auth/logout", Some(&cookie), None).await;
    let (_, me, _) = call(&app, "GET", "/api/auth/me", Some(&cookie), None).await;
    assert!(me["account"].is_null());
}

#[tokio::test]
async fn public_reads_but_writes_need_a_wallet() {
    let (app, s) = app("public");
    s.add_program(PUMP.into(), None).unwrap();
    let (st, body, _) = call(&app, "GET", "/api/status", None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["programs"].as_array().unwrap().len(), 1);
    assert!(body["account"].is_null());

    for (method, path, body) in [
        ("POST", "/api/programs", Some(json!({ "program_id": JUP }))),
        ("DELETE", &*format!("/api/programs/{PUMP}"), None),
        ("GET", "/api/rules", None),
        ("GET", "/api/alerts", None),
        ("PATCH", "/api/incidents/1001", Some(json!({ "status": "resolved" }))),
    ] {
        let (st, _, _) = call(&app, method, path, None, body).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{method} {path}");
    }
}

#[tokio::test]
async fn watchlists_and_rules_are_per_account() {
    let (app, s) = app("owners");
    let (alice, bob) = (Keypair::new(), Keypair::new());
    let (ca, cb) = (sign_in(&app, &alice).await, sign_in(&app, &bob).await);

    // Both watch Pump.fun; only Alice watches Jupiter.
    for c in [&ca, &cb] {
        let (st, _, _) = call(&app, "POST", "/api/programs", Some(c), Some(json!({ "program_id": PUMP }))).await;
        assert_eq!(st, StatusCode::OK);
    }
    call(&app, "POST", "/api/programs", Some(&ca), Some(json!({ "program_id": JUP }))).await;
    let (_, me, _) = call(&app, "GET", "/api/auth/me", Some(&cb), None).await;
    assert_eq!(me["watching"], json!([PUMP]));

    // Bob can't target a program he doesn't watch.
    let rule = |pid: &str| json!({ "name": "r", "program_id": pid, "condition": { "type": "metric", "metric": "tps", "op": ">", "value": 1.0, "window_secs": 60 } });
    let (st, _, _) = call(&app, "POST", "/api/rules", Some(&cb), Some(rule(JUP))).await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Alice's rule is invisible and untouchable for Bob.
    let (st, created, _) = call(&app, "POST", "/api/rules", Some(&ca), Some(rule(PUMP))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(created["owner"], alice.pubkey().to_string());
    let id = created["id"].as_i64().unwrap();
    let (_, bobs, _) = call(&app, "GET", "/api/rules", Some(&cb), None).await;
    assert_eq!(bobs, json!([]));
    let (st, _, _) = call(&app, "DELETE", &format!("/api/rules/{id}"), Some(&cb), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Unwatching keeps the program while someone else still watches it...
    call(&app, "DELETE", &format!("/api/programs/{PUMP}"), Some(&cb), None).await;
    assert!(s.programs().iter().any(|p| p.program_id == PUMP));
    // ...and stops monitoring it when the last watcher leaves.
    call(&app, "DELETE", &format!("/api/programs/{JUP}"), Some(&ca), None).await;
    assert!(!s.programs().iter().any(|p| p.program_id == JUP));
}

#[tokio::test]
async fn webhooks_cannot_target_private_networks() {
    for url in [
        "http://127.0.0.1:8080/hook",
        "http://localhost/hook",
        "http://10.1.2.3/hook",
        "http://192.168.0.10/hook",
        "http://169.254.169.254/latest/meta-data",
        "http://[::1]/hook",
    ] {
        assert!(check_webhook_url(url, false).await.is_err(), "{url} should be blocked");
    }
    assert!(check_webhook_url("ftp://example.com/x", false).await.is_err());
    assert!(check_webhook_url("http://127.0.0.1:8080/hook", true).await.is_ok(), "allowed in dev mode");

    let (app, _) = app("ssrf");
    let kp = Keypair::new();
    let c = sign_in(&app, &kp).await;
    let (st, body, _) = call(
        &app,
        "POST",
        "/api/rules",
        Some(&c),
        Some(json!({ "name": "r", "webhook_url": "http://169.254.169.254/x", "condition": { "type": "incident", "kinds": [], "min_severity": "low" } })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("private network"), "{body}");
}

fn tx(n: u64, second: i64, program: &str) -> Arc<VortexTransaction> {
    use chrono::TimeZone;
    use vortex::events::AccountRef;
    Arc::new(VortexTransaction {
        signature: format!("{program}-{n}"),
        slot: n,
        index: 0,
        received_at: Utc.timestamp_opt(second, 0).unwrap(),
        success: true,
        error: None,
        fee: 5000,
        compute_units: Some(1000),
        compute_unit_limit: None,
        compute_unit_price: None,
        accounts: vec![
            AccountRef { pubkey: "Payer1111111111111111111111111111111111111".into(), signer: true, writable: true, from_lookup_table: false, pre_lamports: 0, post_lamports: 0 },
            AccountRef { pubkey: program.into(), signer: false, writable: false, from_lookup_table: false, pre_lamports: 0, post_lamports: 0 },
        ],
        instructions: vec![],
        invocations: vec![],
        logs: vec![],
        logs_truncated: false,
        token_balances: vec![],
        transfers: vec![],
        filters: vec![],
    })
}

#[tokio::test]
async fn rules_fire_only_on_their_owners_programs() {
    let (app, s) = app("firing");
    let alice = Keypair::new();
    let ca = sign_in(&app, &alice).await;
    call(&app, "POST", "/api/programs", Some(&ca), Some(json!({ "program_id": PUMP }))).await;
    // Someone else's program that Alice doesn't watch.
    s.add_program(JUP.into(), None).unwrap();
    let (st, _, _) = call(
        &app,
        "POST",
        "/api/rules",
        Some(&ca),
        Some(json!({ "name": "busy", "condition": { "type": "metric", "metric": "tps", "op": ">", "value": 1.0, "window_secs": 10 } })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let t0 = 1_700_000_000i64;
    let mut n = 0;
    for sec in t0..t0 + 15 {
        for _ in 0..5 {
            n += 1;
            s.on_transaction(tx(n, sec, PUMP));
            s.on_transaction(tx(n, sec, JUP));
        }
        s.on_tick(sec + 1);
    }
    let fired: Vec<String> = s
        .store
        .incidents(None, 20)
        .unwrap()
        .into_iter()
        .filter(|i| i.source.starts_with("rule:"))
        .map(|i| i.program_id)
        .collect();
    assert_eq!(fired, vec![PUMP.to_string()], "Alice's all-programs rule stays off Jupiter");
}
