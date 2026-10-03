//! Mirage set up by itself, against a stand-in for Solami's management API.

use axum::{extract::State, http::{HeaderMap, StatusCode}, routing::post, Json, Router};
use sentinel::mirage_setup::Mirage;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Api {
    calls: Arc<Mutex<Vec<(String, Value)>>>,
    existing: Arc<Mutex<Value>>,
    forbid: bool,
}

async fn serve(api: Api) -> String {
    async fn list(State(a): State<Api>, h: HeaderMap) -> (StatusCode, Json<Value>) {
        a.calls.lock().unwrap().push(("/mirage/list".into(), json!({ "key": h.get("x-api-key").and_then(|v| v.to_str().ok()) })));
        if a.forbid {
            return (StatusCode::FORBIDDEN, Json(json!({ "message": "missing required permission: MirageView" })));
        }
        (StatusCode::OK, Json(a.existing.lock().unwrap().clone()))
    }
    async fn create(State(a): State<Api>, Json(b): Json<Value>) -> Json<Value> {
        a.calls.lock().unwrap().push(("/mirage/create".into(), b));
        Json(json!({ "id": "sub-123", "label": "sentinel", "enabled": true }))
    }
    async fn update(State(a): State<Api>, Json(b): Json<Value>) -> Json<Value> {
        a.calls.lock().unwrap().push(("/mirage/update".into(), b));
        Json(json!({ "id": "sub-123" }))
    }
    let app = Router::new()
        .route("/mirage/list", post(list))
        .route("/mirage/create", post(create))
        .route("/mirage/update", post(update))
        .with_state(api);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

fn progs(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|s| s.to_string()).collect()
}

fn paths(a: &Api) -> Vec<String> {
    a.calls.lock().unwrap().iter().map(|(p, _)| p.clone()).collect()
}

#[tokio::test]
async fn creates_the_subscription_once_and_builds_the_stream_url() {
    let api = Api { existing: Arc::new(Mutex::new(json!([]))), ..Default::default() };
    let base = serve(api.clone()).await;
    let m = Mirage::new(&base, "wss://ws.example", "KEY12345");
    let url = m.ensure(&progs(&["ProgA", "ProgB"])).await.unwrap();
    assert_eq!(url, "wss://ws.example/mirage/stream/sub-123?api_key=KEY12345");
    assert_eq!(paths(&api), ["/mirage/list", "/mirage/create"]);
    let created = api.calls.lock().unwrap()[1].1.clone();
    assert_eq!(created["label"], "sentinel");
    assert_eq!(created["filter"]["account_include"], json!(["ProgA", "ProgB"]));
    assert_eq!(created["filter"]["failed"], true, "failed transactions are what Sentinel watches for");
    assert_eq!(created["filter"]["vote"], false);
    assert_eq!(api.calls.lock().unwrap()[0].1["key"], "KEY12345", "the key is sent as x-api-key");
}

#[tokio::test]
async fn reuses_an_existing_subscription_however_the_list_is_wrapped() {
    for listing in [
        json!([{ "id": "old-1", "label": "other" }, { "id": "mine-9", "label": "sentinel" }]),
        json!({ "subscriptions": [{ "id": "mine-9", "label": "sentinel" }] }),
    ] {
        let api = Api { existing: Arc::new(Mutex::new(listing)), ..Default::default() };
        let base = serve(api.clone()).await;
        let url = Mirage::new(&base, "wss://ws.example", "K").ensure(&progs(&["ProgA"])).await.unwrap();
        assert!(url.contains("/mirage/stream/mine-9?"), "{url}");
        assert_eq!(paths(&api), ["/mirage/list", "/mirage/update"], "no second subscription is created");
    }
}

#[tokio::test]
async fn follows_the_watchlist_and_only_updates_when_it_changes() {
    let api = Api { existing: Arc::new(Mutex::new(json!([]))), ..Default::default() };
    let base = serve(api.clone()).await;
    let m = Mirage::new(&base, "wss://ws.example", "K");
    m.ensure(&progs(&["B", "A"])).await.unwrap();
    assert!(!m.sync(&progs(&["A", "B"])).await.unwrap(), "same programs in another order: nothing to do");
    assert!(m.sync(&progs(&["A", "B", "C"])).await.unwrap());
    let last = api.calls.lock().unwrap().last().unwrap().clone();
    assert_eq!(last.0, "/mirage/update");
    assert_eq!(last.1["id"], "sub-123");
    assert_eq!(last.1["filter"]["account_include"], json!(["A", "B", "C"]));
    assert!(!m.sync(&progs(&[])).await.unwrap(), "an empty watchlist is never sent");
}

#[tokio::test]
async fn a_key_without_permission_stays_off_and_creates_nothing() {
    let api = Api { forbid: true, existing: Arc::new(Mutex::new(json!([]))), ..Default::default() };
    let base = serve(api.clone()).await;
    let err = Mirage::new(&base, "wss://ws.example", "K").ensure(&progs(&["A"])).await.unwrap_err();
    assert!(err.to_string().contains("HTTP 403") && err.to_string().contains("MirageView"), "{err}");
    assert_eq!(paths(&api), ["/mirage/list"], "nothing is created without permission");
}
