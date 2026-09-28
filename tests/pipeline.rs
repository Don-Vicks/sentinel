//! Drives the real engine with synthetic transactions: baseline → failure
//! spike → incident with linked transactions and fingerprints → rule webhook.

use axum::{routing::post, Json, Router};
use chrono::{TimeZone, Utc};
use sentinel::engine::Sentinel;
use sentinel::model::*;
use sentinel::source::VortexSource;
use sentinel::store::Store;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use vortex::events::logs::Invocation;
use vortex::events::{AccountRef, TxError, VortexTransaction};
use vortex::hub::HubStats;

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

fn tx(n: u64, second: i64, ok: bool) -> Arc<VortexTransaction> {
    let payer = format!("Payer{:0>39}", n % 50);
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
        accounts: vec![
            AccountRef { pubkey: payer, signer: true, writable: true, from_lookup_table: false, pre_lamports: 10, post_lamports: 5 },
            AccountRef { pubkey: PROGRAM.into(), signer: false, writable: false, from_lookup_table: false, pre_lamports: 1, post_lamports: 1 },
        ],
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

#[tokio::test]
async fn failure_spike_becomes_incident_and_fires_webhook() {
    // Local webhook receiver.
    let received: Arc<Mutex<Vec<Value>>> = Arc::default();
    let sink = received.clone();
    let app = Router::new().route(
        "/hook",
        post(move |Json(v): Json<Value>| {
            let sink = sink.clone();
            async move {
                sink.lock().unwrap().push(v);
                "ok"
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let dir = std::env::temp_dir().join(format!("sentinel-test-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    let store = Arc::new(Store::open(dir.to_str().unwrap()).unwrap());
    let (bus, _) = broadcast::channel(16);
    let s = Sentinel::new(store.clone(), Arc::new(FakeSource(bus)), None, sentinel::pricing::PriceBook::new(), "http://ui".into()).unwrap();
    s.add_program(PROGRAM.into(), None).unwrap();
    store
        .create_rule(AlertRule {
            id: 0,
            name: "Page on incidents".into(),
            program_id: None,
            condition: Condition::Incident { kinds: vec![], min_severity: Severity::Medium },
            create_incident: false,
            severity: Severity::High,
            webhook_url: Some(format!("http://{addr}/hook")),
            enabled: true,
            cooldown_secs: 0,
            created_at: Utc::now(),
            last_fired_at: None,
        })
        .unwrap();
    s.reload_rules().unwrap();

    let t0 = 1_700_000_000i64;
    let mut n = 0;
    // 5 minutes of healthy traffic: 5 tx/s, 1 in 50 failing.
    for sec in t0..t0 + 300 {
        for _ in 0..5 {
            n += 1;
            s.on_transaction(tx(n, sec, n % 50 != 0));
        }
        s.on_tick(sec + 1);
    }
    assert!(store.incidents(None, 10).unwrap().is_empty(), "no incident on healthy traffic");

    // 30s where 3 of 5 transactions fail.
    for sec in t0 + 300..t0 + 330 {
        for i in 0..5 {
            n += 1;
            s.on_transaction(tx(n, sec, i >= 3));
        }
        s.on_tick(sec + 1);
    }

    let incidents = store.incidents(Some(PROGRAM), 10).unwrap();
    let inc = incidents
        .iter()
        .find(|i| i.kind == IncidentKind::FailureSpike)
        .expect("failure spike incident");
    assert_eq!(inc.status, IncidentStatus::Open);
    assert!(inc.affected_count >= 50, "linked {} failed txs", inc.affected_count);
    let linked = store.incident_transactions(inc.id, 500).unwrap();
    assert!(linked.iter().all(|t| !t.success));
    let top = &inc.evidence["fingerprints"][0];
    assert_eq!(top["error"], "TooLittleSolReceived");
    assert_eq!(top["instruction"], "Sell");
    assert!(inc.explanation.contains("Threshold"));

    // Recovery: healthy traffic resolves it after the quiet period.
    for sec in t0 + 330..t0 + 600 {
        for _ in 0..5 {
            n += 1;
            s.on_transaction(tx(n, sec, true));
        }
        s.on_tick(sec + 1);
    }
    let resolved = store.incident(inc.id).unwrap().unwrap();
    assert_eq!(resolved.status, IncidentStatus::Resolved);

    // A second spike is judged against a baseline that excludes the first.
    for sec in t0 + 600..t0 + 630 {
        for i in 0..5 {
            n += 1;
            s.on_transaction(tx(n, sec, i >= 3));
        }
        s.on_tick(sec + 1);
    }
    let second = store
        .incidents(Some(PROGRAM), 10)
        .unwrap()
        .into_iter()
        .find(|i| i.kind == IncidentKind::FailureSpike && i.id != inc.id)
        .expect("second failure spike");
    assert!(second.baseline.unwrap() < 5.0, "baseline {:?} polluted", second.baseline);

    // Webhook delivered for the incident.
    for _ in 0..50 {
        if !received.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let hooks = received.lock().unwrap();
    assert!(hooks.iter().any(|h| h["event"] == "sentinel.incident"));
    assert!(store.executions(10).unwrap().iter().any(|e| e.delivered));
    let _ = std::fs::remove_file(&dir);
}

fn with_error(base: Arc<VortexTransaction>, name: &str, code: u32) -> Arc<VortexTransaction> {
    let mut t = (*base).clone();
    if let Some(e) = t.error.as_mut() {
        e.name = Some(name.into());
        e.custom_code = Some(code);
    }
    Arc::new(t)
}

#[tokio::test]
async fn new_error_type_opens_incident_while_failure_rate_is_flat() {
    let dir = std::env::temp_dir().join(format!("sentinel-errspike-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    let store = Arc::new(Store::open(dir.to_str().unwrap()).unwrap());
    let (bus, _) = broadcast::channel(16);
    let s = Sentinel::new(store.clone(), Arc::new(FakeSource(bus)), None, sentinel::pricing::PriceBook::new(), "http://ui".into()).unwrap();
    s.add_program(PROGRAM.into(), None).unwrap();

    let t0 = 1_700_000_000i64;
    let mut n = 0;
    // 5 min at 5 tx/s with a steady 10% slippage failures.
    for sec in t0..t0 + 300 {
        for _ in 0..5 {
            n += 1;
            let t = tx(n, sec, n % 10 != 0);
            s.on_transaction(if t.success { t } else { with_error(t, "TooLittleSolReceived", 6003) });
        }
        s.on_tick(sec + 1);
    }
    // Next 60s: same 10% failure rate, but half of it is a brand-new error.
    for sec in t0 + 300..t0 + 360 {
        for _ in 0..5 {
            n += 1;
            let t = tx(n, sec, n % 10 != 0);
            let t = match (t.success, n % 20 == 0) {
                (true, _) => t,
                (false, true) => with_error(t, "AccountNotInitialized", 3012),
                (false, false) => with_error(t, "TooLittleSolReceived", 6003),
            };
            s.on_transaction(t);
        }
        s.on_tick(sec + 1);
    }
    let incidents = store.incidents(Some(PROGRAM), 10).unwrap();
    assert!(incidents.iter().all(|i| i.kind != IncidentKind::FailureSpike), "overall rate is flat");
    let inc = incidents
        .iter()
        .find(|i| i.kind == IncidentKind::ErrorSpike)
        .expect("error spike incident");
    assert!(inc.title.starts_with("New error"), "{}", inc.title);
    assert!(inc.summary.contains("AccountNotInitialized"), "{}", inc.summary);
    let linked = store.incident_transactions(inc.id, 100).unwrap();
    assert!(!linked.is_empty());
    assert!(linked.iter().all(|t| t.error.as_deref() == Some("AccountNotInitialized")));
    let _ = std::fs::remove_file(&dir);
}
