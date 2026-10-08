//! Rule suggestions read from Pump.fun's real on-chain IDL, and applying them.

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
use vortex::geyser::decode::decode_transaction;
use vortex::geyser::rpc_frame::frame_from_rpc_json;
use vortex::hub::HubStats;


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


const PUMP: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";

fn pump_idl() -> sentinel::idl::Idl {
    let data = std::fs::read(format!("{}/tests/fixtures/pump_idl_account.bin", env!("CARGO_MANIFEST_DIR"))).unwrap();
    sentinel::idl::Idl::parse(PUMP, &sentinel::idl::decode_idl_account(&data).unwrap()).unwrap()
}

#[tokio::test]
async fn suggestions_come_from_the_idl_and_apply_once() {
    let db = std::env::temp_dir().join(format!("sentinel-suggest-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let store = Arc::new(Store::open(db.to_str().unwrap()).unwrap());
    let (bus, _) = broadcast::channel(4);
    let s = Sentinel::new(store.clone(), Arc::new(NullSource(bus)), None, PriceBook::new(), "https://sentinel.example".into()).unwrap();
    s.add_program(PUMP.into(), None).unwrap();
    let app = sentinel::api::router(s.clone());
    let cookie = sign_in(&app, &Keypair::new()).await;

    // Without an IDL there is nothing to read.
    let (st, none, _) = call(&app, "GET", &format!("/api/programs/{PUMP}/suggestions"), None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(none["idl_loaded"], false);

    s.idls.insert(pump_idl());
    let (_, got, _) = call(&app, "GET", &format!("/api/programs/{PUMP}/suggestions"), None, None).await;
    assert_eq!(got["idl_loaded"], true);
    let list = got["suggestions"].as_array().unwrap();
    let ids: Vec<&str> = list.iter().map(|x| x["id"].as_str().unwrap()).collect();
    println!("{ids:?}");
    let find = |id: &str| list.iter().find(|x| x["id"] == id).unwrap_or_else(|| panic!("{id} not in {ids:?}"));
    let authority = find("instruction:authority");
    assert_eq!(authority["severity"], "critical");
    assert!(authority["matches"].to_string().contains("update_global_authority"), "{authority}");
    assert!(find("instruction:config")["matches"].to_string().contains("set_params"));
    assert!(ids.iter().any(|i| i.starts_with("event:")), "Pump.fun declares events: {ids:?}");
    assert!(list.iter().all(|x| !x["why"].as_str().unwrap().is_empty()));

    // Applying needs an account that watches the program.
    let body = json!({ "ids": ["instruction:authority"], "channels": [{ "type": "slack", "url": "https://hooks.slack.com/services/T/B/x" }] });
    let (st, _, _) = call(&app, "POST", &format!("/api/programs/{PUMP}/suggestions"), None, Some(body.clone())).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, watch, _) = call(&app, "POST", "/api/programs", Some(&cookie), Some(json!({ "program_id": PUMP }))).await;
    assert!(st.is_success(), "{st} {watch}");

    let (st, applied, _) = call(&app, "POST", &format!("/api/programs/{PUMP}/suggestions"), Some(&cookie), Some(body.clone())).await;
    assert_eq!(st, StatusCode::OK, "{applied}");
    assert_eq!(applied["created"].as_array().unwrap().len(), 1, "{applied}");
    let rule = &applied["created"][0];
    assert_eq!(rule["severity"], "critical");
    assert_eq!(rule["condition"]["type"], "instruction");
    assert!(rule["condition"]["name"].as_str().unwrap().contains("update_global_authority"));
    assert_eq!(rule["program_id"], PUMP);

    // Again: nothing new.
    let (_, again, _) = call(&app, "POST", &format!("/api/programs/{PUMP}/suggestions"), Some(&cookie), Some(body)).await;
    assert!(again["created"].as_array().unwrap().is_empty());
    assert_eq!(again["already_had"].as_array().unwrap().len(), 1);

    // A size threshold has to be chosen: a program whose withdraw takes an amount.
    s.idls.insert(
        sentinel::idl::Idl::parse(
            PUMP,
            &json!({
                "metadata": { "name": "vault", "spec": "0.1.0" },
                "instructions": [{ "name": "withdraw", "discriminator": [2,0,0,0,0,0,0,0], "accounts": [], "args": [{ "name": "amount", "type": "u64" }] }]
            }),
        )
        .unwrap(),
    );
    let large = "instruction:large:withdraw";
    let chan = json!([{ "type": "slack", "url": "https://hooks.slack.com/services/T/B/x" }]);
    let (st, err, _) = call(&app, "POST", &format!("/api/programs/{PUMP}/suggestions"), Some(&cookie), Some(json!({ "ids": [large], "channels": chan }))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{err}");
    assert!(err.to_string().contains("needs a value"), "{err}");
    let (st, ok, _) = call(&app, "POST", &format!("/api/programs/{PUMP}/suggestions"), Some(&cookie), Some(json!({ "ids": [large], "values": { large: 5_000_000_000u64 }, "channels": chan }))).await;
    assert_eq!(st, StatusCode::OK, "{ok}");
    assert_eq!(ok["created"][0]["condition"]["filters"][0]["value"], 5_000_000_000u64);
    assert_eq!(ok["created"][0]["condition"]["filters"][0]["path"], "args.amount");
    // An id that isn't a suggestion is refused.
    let (st, _, _) = call(&app, "POST", &format!("/api/programs/{PUMP}/suggestions"), Some(&cookie), Some(json!({ "ids": ["nope"], "channels": [{ "type": "slack", "url": "https://hooks.slack.com/services/T/B/x" }] }))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let _ = std::fs::remove_file(&db);
}

fn fixture(name: &str) -> VortexTransaction {
    let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let f: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let resp = &f["response"];
    let frame = frame_from_rpc_json(f["signature"].as_str().unwrap(), resp["slot"].as_u64().unwrap(), resp).unwrap();
    decode_transaction(frame, vec![]).unwrap()
}

#[tokio::test]
async fn an_idl_you_supply_decodes_events_and_both_survive_a_restart() {
    let db = std::env::temp_dir().join(format!("sentinel-custom-idl-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let make = || {
        let store = Arc::new(Store::open(db.to_str().unwrap()).unwrap());
        let (bus, _) = broadcast::channel(4);
        Sentinel::new(store, Arc::new(NullSource(bus)), None, PriceBook::new(), "https://sentinel.example".into()).unwrap()
    };
    let s = make();
    s.add_program(PUMP.into(), None).unwrap();
    let app = sentinel::api::router(s.clone());
    let cookie = sign_in(&app, &Keypair::new()).await;
    let stranger = sign_in(&app, &Keypair::new()).await;
    let (st, _, _) = call(&app, "POST", "/api/programs", Some(&cookie), Some(json!({ "program_id": PUMP }))).await;
    assert!(st.is_success());

    // Nothing on chain (no RPC here) and nothing supplied: events can't be named.
    s.on_transaction(Arc::new(fixture("pump_ok")));
    s.flush();
    let (_, none, _) = call(&app, "GET", &format!("/api/programs/{PUMP}/events"), None, None).await;
    assert_eq!((none["idl_loaded"].as_bool(), none["events"].as_array().unwrap().len()), (Some(false), 0));

    // Supplying one needs an account that watches the program, and a real IDL.
    let data = std::fs::read(format!("{}/tests/fixtures/pump_idl_account.bin", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let idl_json = sentinel::idl::decode_idl_account(&data).unwrap();
    let uri = format!("/api/programs/{PUMP}/idl");
    assert_eq!(call(&app, "PUT", &uri, None, Some(idl_json.clone())).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&app, "PUT", &uri, Some(&stranger), Some(idl_json.clone())).await.0, StatusCode::FORBIDDEN, "not watching");
    let (st, bad, _) = call(&app, "PUT", &uri, Some(&cookie), Some(json!({ "hello": "world" }))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{bad}");
    assert!(bad.to_string().contains("not an Anchor IDL"), "{bad}");
    let (st, ok, _) = call(&app, "PUT", &uri, Some(&cookie), Some(idl_json)).await;
    assert_eq!(st, StatusCode::OK, "{ok}");
    assert!(ok["events"].as_u64().unwrap() > 10 && ok["instructions"].as_u64().unwrap() > 10);
    let (_, shown, _) = call(&app, "GET", &uri, None, None).await;
    assert_eq!((shown["loaded"].as_bool(), shown["custom"].as_bool()), (Some(true), Some(true)));

    // Events from now on are decoded and kept.
    s.on_transaction(Arc::new(VortexTransaction { signature: "again".into(), ..fixture("pump_ok") }));
    s.flush();
    let (_, got, _) = call(&app, "GET", &format!("/api/programs/{PUMP}/events"), None, None).await;
    assert_eq!(got["custom_idl"], true);
    assert_eq!(got["events"].as_array().unwrap().len(), 1, "{got}");
    assert_eq!(got["events"][0]["name"], "TradeEvent");
    assert_eq!(got["names"][0]["name"], "TradeEvent");
    assert_eq!(got["names"][0]["count"], 1);
    let (_, filtered, _) = call(&app, "GET", &format!("/api/programs/{PUMP}/events?name=CreateEvent"), None, None).await;
    assert!(filtered["events"].as_array().unwrap().is_empty());
    drop(app);
    drop(s);

    // A restart: the IDL and the event are still there.
    let s2 = make();
    assert!(s2.idls.is_custom(PUMP) && s2.idls.cached(PUMP).is_some());
    assert_eq!(s2.recent_events(PUMP, None, 10).len(), 1);

    // Removing it goes back to nothing (there is no chain here).
    let app2 = sentinel::api::router(s2.clone());
    let cookie2 = sign_in(&app2, &Keypair::new()).await;
    let _ = call(&app2, "POST", "/api/programs", Some(&cookie2), Some(json!({ "program_id": PUMP }))).await;
    let (st, gone, _) = call(&app2, "DELETE", &uri, Some(&cookie2), None).await;
    assert_eq!(st, StatusCode::OK, "{gone}");
    assert_eq!(gone["removed"], true);
    assert!(!s2.idls.is_custom(PUMP));
    let _ = std::fs::remove_file(&db);
}
