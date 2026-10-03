//! An RPC failure must never put the Solami API key in a response, however the client words
//! its error. Regression test: the transaction page once showed `...rpc.solami.dev/sol?api_key=...`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use sentinel::engine::Sentinel;
use sentinel::pricing::PriceBook;
use sentinel::source::VortexSource;
use sentinel::store::Store;
use solana_client::nonblocking::rpc_client::RpcClient;
use std::sync::Arc;
use tokio::sync::broadcast;
use tower::ServiceExt;
use vortex::events::VortexTransaction;
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

#[tokio::test]
async fn rpc_errors_do_not_leak_the_api_key() {
    const SECRET: &str = "rpc_SUPERSECRETKEY123";
    std::env::set_var("SOLANA_RPC_URL", format!("http://127.0.0.1:1/sol?api_key={SECRET}"));
    let db = std::env::temp_dir().join(format!("sentinel-redact-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let store = Arc::new(Store::open(db.to_str().unwrap()).unwrap());
    let (bus, _) = broadcast::channel(4);
    let rpc = Arc::new(RpcClient::new(format!("http://127.0.0.1:1/sol?api_key={SECRET}")));
    let s = Sentinel::new(store, Arc::new(NullSource(bus)), Some(rpc), PriceBook::new(), "http://localhost".into()).unwrap();
    let app = sentinel::api::router(s);

    // A valid-looking signature that is not in the live window, so Sentinel asks the (dead) RPC.
    let sig = bs58::encode([9u8; 64]).into_string();
    let res = app
        .oneshot(Request::builder().uri(format!("/api/transactions/{sig}")).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_ne!(res.status(), StatusCode::OK);
    let body = String::from_utf8(axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap().to_vec()).unwrap();
    assert!(!body.contains(SECRET), "the API key leaked: {body}");
    assert!(!body.contains("api_key"), "key parameter leaked: {body}");
    assert!(body.contains("error"), "{body}");
}
