//! Blur pricing: parsing the documented response, and USD in detection/traces.

use axum::http::HeaderMap;
use axum::{routing::post, Json, Router};
use chrono::Utc;
use sentinel::engine::Sentinel;
use sentinel::model::*;
use sentinel::pricing::{PriceBook, WSOL};
use sentinel::source::VortexSource;
use sentinel::store::Store;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use vortex::events::{AccountRef, Transfer, TransferKind, VortexTransaction};
use vortex::hub::HubStats;

const PROGRAM: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";
const MEME: &str = "2uqgcWq1Vv6XuQvZ4U4N3EMpbL3wiBnZaTyj7ZKmpump";
const USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

#[tokio::test]
async fn parses_blur_price_response() {
    let seen: Arc<Mutex<Vec<(Option<String>, Value)>>> = Arc::default();
    let sink = seen.clone();
    let app = Router::new().route(
        "/data/token/price",
        post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let sink = sink.clone();
            async move {
                let key = headers.get("x-api-key").map(|v| v.to_str().unwrap().to_string());
                sink.lock().unwrap().push((key, body));
                // Shape from the Solami docs: decimals as strings, SOL venue-less.
                Json(json!([
                    { "mint": USDC, "price_usd": "1.0001", "price_native": "0.0055", "quote_mint": WSOL,
                      "pool": "p", "block_time": 1, "liquidity_usd": "22007257.36" },
                    { "mint": WSOL, "price_usd": "181.25", "price_native": "1", "quote_mint": "",
                      "pool": "", "block_time": 1, "liquidity_usd": "0" },
                    { "mint": MEME, "price_usd": "0.0000008672", "price_native": "0.000000005", "quote_mint": WSOL,
                      "pool": "p", "block_time": 1, "liquidity_usd": "3200.5" }
                ]))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let book = PriceBook::new();
    book.spawn(format!("http://{addr}"), "test-key".into());
    book.note(Some(MEME));
    for _ in 0..60 {
        if book.get(Some(MEME)).is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    let calls = seen.lock().unwrap().clone();
    assert_eq!(calls[0].0.as_deref(), Some("test-key"));
    assert_eq!(calls[0].1["chain"], "solana");
    assert_eq!(calls[0].1["liquidity"], true);

    assert!((book.usd(None, 2.0).unwrap() - 362.5).abs() < 1e-9, "native SOL priced via wSOL");
    assert!(book.get(None).unwrap().trusted(), "SOL is always trusted");
    assert!(book.get(Some(USDC)).unwrap().trusted());
    let meme = book.get(Some(MEME)).unwrap();
    assert!((meme.usd - 0.0000008672).abs() < 1e-15);
    assert!(!meme.trusted(), "thin liquidity is not trusted for alerts");
    let status = book.status();
    assert!(status.enabled && status.priced_mints == 3 && status.last_error.is_none());
}

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

fn transfer_tx(sig: &str, mint: Option<&str>, amount: f64) -> Arc<VortexTransaction> {
    let payer = "Payer1111111111111111111111111111111111111";
    Arc::new(VortexTransaction {
        signature: sig.into(),
        slot: 1,
        index: 0,
        received_at: Utc::now(),
        success: true,
        error: None,
        fee: 5000,
        compute_units: Some(10_000),
        compute_unit_limit: None,
        compute_unit_price: None,
        accounts: vec![
            AccountRef { pubkey: payer.into(), signer: true, writable: true, from_lookup_table: false, pre_lamports: 0, post_lamports: 0 },
            AccountRef { pubkey: PROGRAM.into(), signer: false, writable: false, from_lookup_table: false, pre_lamports: 0, post_lamports: 0 },
        ],
        instructions: vec![],
        invocations: vec![],
        logs: vec![],
        logs_truncated: false,
        token_balances: vec![],
        transfers: vec![Transfer {
            kind: if mint.is_some() { TransferKind::Token } else { TransferKind::Sol },
            mint: mint.map(str::to_string),
            from: Some(payer.into()),
            to: Some("Vault111111111111111111111111111111111111111".into()),
            from_owner: Some(payer.into()),
            to_owner: Some("Vault111111111111111111111111111111111111111".into()),
            authority: None,
            amount_raw: 0,
            decimals: 6,
            amount,
            instruction: "0".into(),
        }],
        filters: vec![],
    })
}

#[tokio::test]
async fn usd_large_transfer_detection_and_rule() {
    let db = std::env::temp_dir().join(format!("sentinel-pricing-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let store = Arc::new(Store::open(db.to_str().unwrap()).unwrap());
    let prices = PriceBook::new();
    prices.insert(WSOL, 180.0, 0.0);
    prices.insert(MEME, 0.001, 3_000.0); // thin
    prices.insert(USDC, 1.0, 50_000_000.0);
    let (bus, _) = broadcast::channel(4);
    let s = Sentinel::new(store.clone(), Arc::new(FakeSource(bus)), None, prices, "http://ui".into()).unwrap();
    s.add_program(PROGRAM.into(), None).unwrap();

    // 1B meme tokens "worth" $1M at a thin price: shown, but no incident.
    s.on_transaction(transfer_tx("meme", Some(MEME), 1_000_000_000.0));
    assert!(store.incidents(None, 10).unwrap().is_empty());
    let feed = s.recent_transactions(PROGRAM, false, 1);
    assert!((feed[0].largest_transfer.as_ref().unwrap().usd.unwrap() - 1_000_000.0).abs() < 1e-6);

    // 1,500 SOL = $270K: under the 500 SOL amount rule? No, above it (3×),
    // and above the $250K USD rule (1.08×). The bigger multiple wins.
    s.on_transaction(transfer_tx("sol", None, 1_500.0));
    let inc = store.incidents(None, 10).unwrap().into_iter().next().expect("large transfer");
    assert_eq!(inc.kind, IncidentKind::LargeTransfer);
    assert!(inc.summary.contains("$270"), "{}", inc.summary);
    assert_eq!(inc.metric.as_deref(), Some("transfer_amount"));

    // A USD rule fires on a liquid token.
    store
        .create_rule(AlertRule {
            id: 0,
            name: "Whale".into(),
            program_id: None,
            condition: Condition::TransferUsd { min_usd: 100_000.0 },
            create_incident: true,
            severity: Severity::High,
            webhook_url: None,
            enabled: true,
            cooldown_secs: 0,
            created_at: Utc::now(),
            last_fired_at: None,
        })
        .unwrap();
    s.reload_rules().unwrap();
    s.on_transaction(transfer_tx("usdc", Some(USDC), 150_000.0));
    let rule_inc = store
        .incidents(None, 10)
        .unwrap()
        .into_iter()
        .find(|i| i.kind == IncidentKind::RuleTriggered)
        .expect("usd rule incident");
    assert!(rule_inc.summary.contains("USDC") && rule_inc.summary.contains("$150"), "{}", rule_inc.summary);
    assert_eq!(store.incident_transactions(rule_inc.id, 10).unwrap()[0].signature, "usdc");
    let _ = std::fs::remove_file(&db);
}
