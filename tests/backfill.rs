//! Loading a program's recent history over RPC, so detectors have a baseline at once.

use chrono::Utc;
use sentinel::engine::Sentinel;
use sentinel::source::VortexSource;
use sentinel::store::Store;
use axum::{extract::State, routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::broadcast;
use vortex::events::VortexTransaction;
use vortex::geyser::decode::decode_transaction;
use vortex::geyser::rpc_frame::frame_from_rpc_json;
use vortex::hub::HubStats;

const PUMP: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";

struct FakeSource(broadcast::Sender<Arc<VortexTransaction>>);
impl VortexSource for FakeSource {
    fn subscribe(&self) -> broadcast::Receiver<Arc<VortexTransaction>> {
        self.0.subscribe()
    }
    fn watch_programs(&self, _: Vec<String>) {}
    fn health(&self) -> HubStats {
        HubStats { transactions: 0, last_slot: 0, last_transaction_at: None, started_at: Utc::now(), programs: vec![], subscribers: 0 }
    }
}

fn fixture(name: &str) -> VortexTransaction {
    let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let f: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let resp = &f["response"];
    let frame = frame_from_rpc_json(f["signature"].as_str().unwrap(), resp["slot"].as_u64().unwrap(), resp).unwrap();
    decode_transaction(frame, vec![]).unwrap()
}


struct Mock {
    ok: Value,
    failed: Value,
    now: i64,
    pages: AtomicUsize,
}

async fn rpc(State(m): State<Arc<Mock>>, Json(req): Json<Value>) -> Json<Value> {
    let id = req["id"].clone();
    let result = match req["method"].as_str().unwrap_or_default() {
        "getSignaturesForAddress" => {
            // One page of 50, newest first, then nothing.
            if m.pages.fetch_add(1, Ordering::SeqCst) > 0 {
                json!([])
            } else {
                json!((0..50).rev().map(|i| json!({
                    "signature": sig_for(i), "slot": 1000 + i, "err": null, "memo": null,
                    "blockTime": m.now - 600 + i * 10, "confirmationStatus": "confirmed"
                })).collect::<Vec<_>>())
            }
        }
        "getTransaction" => {
            let sig = req["params"][0].as_str().unwrap_or_default();
            let i: i64 = bs58::decode(sig.trim_start_matches("Sg")).into_vec().ok().and_then(|b| b.first().copied()).unwrap_or(0) as i64;
            let mut tx = if i % 5 == 0 { m.failed.clone() } else { m.ok.clone() };
            tx["blockTime"] = json!(m.now - 600 + i * 10);
            tx
        }
        // The Solana client asks which version it is talking to before anything else.
        "getVersion" => json!({ "solana-core": "1.18.26", "feature-set": 1 }),
        _ => Value::Null,
    };
    Json(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

/// A valid base58 signature that carries its index.
fn sig_for(i: i64) -> String {
    format!("Sg{}", bs58::encode([i as u8]).into_string())
}

fn response(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let f: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    f["response"].clone()
}

#[tokio::test]
async fn recent_history_gives_a_program_a_baseline_without_raising_alerts() {
    let now = Utc::now().timestamp();
    let mock = Arc::new(Mock { ok: response("pump_ok"), failed: response("pump_failed"), now, pages: AtomicUsize::new(0) });
    let app = Router::new().route("/", post(rpc)).with_state(mock);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let dir = std::env::temp_dir().join(format!("sentinel-backfill-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    let store = Arc::new(Store::open(dir.to_str().unwrap()).unwrap());
    let (bus, _) = broadcast::channel(16);
    let rpc = Arc::new(solana_client::nonblocking::rpc_client::RpcClient::new(format!("http://{addr}")));
    let s = Sentinel::new(store.clone(), Arc::new(FakeSource(bus)), Some(rpc), sentinel::pricing::PriceBook::new(), "http://ui".into()).unwrap();
    s.add_program(PUMP.into(), None).unwrap();

    // Before: a brand-new program knows nothing and detectors are still learning.
    let before = &s.programs()[0];
    assert_eq!(before.total_tx, 0);
    assert!(before.warmup_remaining_secs > 0);

    let report = s.backfill(PUMP).await.unwrap();
    assert_eq!(report.transactions, 50);
    assert!((report.seconds - 490).abs() <= 1, "covers about 490 seconds: {}", report.seconds);

    // After: it has seen the history, so there is nothing left to learn.
    s.on_tick(now + 1);
    let after = &s.programs()[0];
    assert_eq!(after.total_tx, 50);
    assert_eq!(after.total_failed, 10, "one in five failed");
    assert_eq!(after.warmup_remaining_secs, 0, "armed at once");
    assert!(store.incidents(None, 10).unwrap().is_empty(), "history opens no incident");
    assert!(store.executions(10).unwrap().is_empty(), "and sends no alert");
    // Summaries have the history too.
    let hours = store.rollups_between(PUMP, now - 2 * 3600, now + 3600).unwrap();
    assert_eq!(hours.iter().map(|(_, r)| r.tx).sum::<u64>(), 50);
    assert_eq!(hours.iter().map(|(_, r)| r.failed).sum::<u64>(), 10);

    // Loading it again changes nothing.
    assert_eq!(s.backfill(PUMP).await.unwrap().transactions, 0);
    assert_eq!(s.programs()[0].total_tx, 50);
    let _ = std::fs::remove_file(&dir);
}
