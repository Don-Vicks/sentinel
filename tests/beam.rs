//! Beam lookups against a local stand-in for Solami's API: found, not a Beam
//! transaction, tip detection, caching, and failure handling.

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use chrono::Utc;
use sentinel::beam::{describe, tip_from, BeamClient};
use serde_json::json;
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use vortex::events::{Transfer, TransferKind, VortexTransaction};

const TIP: &str = "15qWd4huAkoxvhDsHMfpUn27TW1YBYMMJJ2jkAkbeam";

async fn serve(hits: Arc<AtomicUsize>) -> String {
    let app = Router::new()
        .route("/onchain/tip-addresses", get(|| async { Json(json!([TIP])) }))
        .route(
            "/swqos/tx/{sig}",
            get(move |Path(sig): Path<String>| {
                hits.fetch_add(1, Ordering::SeqCst);
                async move {
                    if sig == "landed" {
                        Json(json!({"signature": sig, "is_landed": true, "landed_via_jito": true, "region": "NYC",
                                    "tip_lamports": 100000, "first_seen_ms": 5000, "forwarded_ms": 5009}))
                        .into_response()
                    } else if sig == "broken" {
                        (StatusCode::INTERNAL_SERVER_ERROR, "boom").into_response()
                    } else {
                        (StatusCode::NOT_FOUND, Json(json!({"message": "not found!"}))).into_response()
                    }
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

fn tx_with(transfers: Vec<Transfer>) -> VortexTransaction {
    VortexTransaction {
        signature: "s".into(),
        slot: 1,
        index: 0,
        received_at: Utc::now(),
        success: true,
        error: None,
        fee: 5000,
        compute_units: None,
        compute_unit_limit: None,
        compute_unit_price: None,
        accounts: vec![],
        instructions: vec![],
        invocations: vec![],
        logs: vec![],
        logs_truncated: false,
        token_balances: vec![],
        transfers,
        filters: vec![],
    }
}

fn sol(to: &str, lamports: u64) -> Transfer {
    Transfer {
        kind: TransferKind::Sol,
        mint: None,
        from: Some("payer".into()),
        to: Some(to.into()),
        from_owner: None,
        to_owner: None,
        authority: None,
        amount_raw: lamports,
        decimals: 9,
        amount: lamports as f64 / 1e9,
        instruction: "transfer".into(),
    }
}

#[tokio::test]
async fn finds_landing_and_treats_404_as_not_beam() {
    let hits = Arc::new(AtomicUsize::new(0));
    let beam = BeamClient::with_base(serve(hits.clone()).await);

    let l = beam.landing("landed").await.expect("landed via beam");
    assert!(l.is_landed && l.landed_via_jito);
    assert_eq!(l.region.as_deref(), Some("NYC"));
    assert_eq!(l.forward_latency_ms(), Some(9));

    assert!(beam.landing("other").await.is_none(), "404 means not sent through Beam");
    assert!(beam.landing("broken").await.is_none(), "server errors degrade to nothing, not a crash");

    // Results are cached: repeats don't hit the API again (broken isn't cached).
    let before = hits.load(Ordering::SeqCst);
    beam.landing("landed").await;
    beam.landing("other").await;
    assert_eq!(hits.load(Ordering::SeqCst), before);
}

#[tokio::test]
async fn detects_tips_and_unreachable_api_is_harmless() {
    let beam = BeamClient::with_base(serve(Arc::default()).await);
    let tips = beam.tip_addresses().await;
    assert!(tips.contains(TIP));

    let tx = tx_with(vec![sol(TIP, 100_000), sol(TIP, 50_000), sol("someone-else", 9_000_000)]);
    let tip = beam.tip_in(&tx).await.expect("tip found");
    assert_eq!((tip.lamports, tip.address.as_str()), (150_000, TIP));
    assert!(describe(None, Some(&tip)).unwrap().contains("0.00015 SOL"));
    assert!(tip_from(&tx_with(vec![sol("x", 1)]), &tips).is_none());
    // Token transfers named like a tip don't count; only native SOL does.
    let mut token = sol(TIP, 5);
    token.kind = TransferKind::Token;
    assert!(tip_from(&tx_with(vec![token]), &tips).is_none());

    let down = BeamClient::with_base("http://127.0.0.1:1".into());
    assert!(down.landing("landed").await.is_none());
    assert_eq!(down.tip_addresses().await, HashSet::new());
}
