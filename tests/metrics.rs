//! Prometheus metrics: what Sentinel exposes at /metrics.

use chrono::{TimeZone, Utc};
use sentinel::engine::Sentinel;

use sentinel::source::VortexSource;
use sentinel::store::Store;
use std::sync::Arc;
use tokio::sync::broadcast;
use vortex::events::logs::Invocation;
use vortex::events::{AccountRef, TxError, VortexTransaction};
use vortex::hub::HubStats;

const PUMP: &str = PROGRAM;
const PROGRAM: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";

struct FakeSource(broadcast::Sender<Arc<VortexTransaction>>);

impl VortexSource for FakeSource {
    fn subscribe(&self) -> broadcast::Receiver<Arc<VortexTransaction>> {
        self.0.subscribe()
    }
    fn watch_programs(&self, _: Vec<String>) {}
    fn health(&self) -> HubStats {
        HubStats {
            transactions: 0,
            last_slot: 0,
            last_transaction_at: None,
            started_at: Utc::now(),
            programs: vec![],
            subscribers: 0,
        }
    }
}

fn account(pubkey: &str, signer: bool) -> AccountRef {
    AccountRef { pubkey: pubkey.into(), signer, writable: true, from_lookup_table: false, pre_lamports: 10, post_lamports: 5 }
}

/// Ordinary program traffic.
fn traffic(n: u64, second: i64, ok: bool) -> Arc<VortexTransaction> {
    Arc::new(VortexTransaction {
        signature: format!("sig{n}"),
        slot: 1000 + n,
        index: 0,
        received_at: Utc.timestamp_opt(second, 0).unwrap(),
        success: ok,
        error: (!ok).then(|| TxError {
            message: "InstructionError(2, Custom(6003))".into(),
            instruction_index: Some(2),
            custom_code: Some(6003),
            program_id: Some(PROGRAM.into()),
            name: Some("TooLittleSolReceived".into()),
            class: "Unknown".into(),
        }),
        fee: 5000,
        compute_units: Some(40_000),
        compute_unit_limit: None,
        compute_unit_price: None,
        accounts: vec![account(&format!("Payer{:0>39}", n % 50), true), account(PROGRAM, false)],
        instructions: vec![],
        invocations: vec![Invocation {
            program_id: PROGRAM.into(),
            depth: 1,
            instruction: Some("Sell".into()),
            compute_consumed: Some(40_000),
            success: Some(ok),
            failure: (!ok).then(|| "custom program error: 0x1773".into()),
            ..Default::default()
        }],
        logs: vec![],
        logs_truncated: false,
        token_balances: vec![],
        transfers: vec![],
        filters: vec![],
    })
}


async fn scrape(app: &axum::Router, token: Option<&str>) -> (axum::http::StatusCode, String) {
    use tower::ServiceExt;
    let mut req = axum::http::Request::builder().uri("/metrics");
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let res = app.clone().oneshot(req.body(axum::body::Body::empty()).unwrap()).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 22).await.unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

#[tokio::test]
async fn metrics_describe_the_stream_programs_incidents_and_deliveries() {
    let dir = std::env::temp_dir().join(format!("sentinel-metrics-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    let store = Arc::new(Store::open(dir.to_str().unwrap()).unwrap());
    let (bus, _) = broadcast::channel(16);
    let s = Sentinel::new(store.clone(), Arc::new(FakeSource(bus)), None, sentinel::pricing::PriceBook::new(), "http://ui".into()).unwrap();
    s.add_program(PUMP.into(), None).unwrap();
    let t0 = chrono::Utc::now().timestamp() - 120;
    for sec in 0..60 {
        for i in 0..5 {
            s.on_transaction(traffic(sec as u64 * 5 + i, t0 + sec, i != 0));
        }
        s.on_tick(t0 + sec + 1);
    }
    let app = sentinel::api::router(s.clone());
    let (status, text) = scrape(&app, None).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(text.contains("sentinel_up 1\n"), "{text}");
    assert!(text.contains("# TYPE sentinel_program_failure_rate_percent gauge"));
    assert!(text.contains(&format!("sentinel_program_transactions_total{{program=\"{PUMP}\",label=\"Pump.fun\"}} 300")), "{text}");
    assert!(text.contains(&format!("sentinel_program_failed_transactions_total{{program=\"{PUMP}\",label=\"Pump.fun\"}} 60")), "{text}");
    assert!(text.contains("sentinel_stream_stalled 0"), "{text}");
    // Every sample line is `name{labels} number`, and no metric is declared twice.
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        let value = line.rsplit(' ').next().unwrap();
        assert!(value.parse::<f64>().is_ok(), "bad sample {line:?}");
    }
    let types: Vec<&str> = text.lines().filter(|l| l.starts_with("# TYPE ")).collect();
    let unique: std::collections::HashSet<_> = types.iter().collect();
    assert_eq!(types.len(), unique.len(), "a metric is declared once");

    // With a token set, scrapers must send it.
    std::env::set_var("SENTINEL_METRICS_TOKEN", "scrape-me");
    assert_eq!(scrape(&app, None).await.0, axum::http::StatusCode::UNAUTHORIZED);
    assert_eq!(scrape(&app, Some("wrong")).await.0, axum::http::StatusCode::UNAUTHORIZED);
    assert_eq!(scrape(&app, Some("scrape-me")).await.0, axum::http::StatusCode::OK);
    std::env::remove_var("SENTINEL_METRICS_TOKEN");
    let _ = std::fs::remove_file(&dir);
}
