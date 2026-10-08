//! A maintenance window holds notifications for a program, but not the record of what happened.

use axum::extract::Path;
use axum::{routing::post, Json, Router};
use chrono::{TimeZone, Utc};
use sentinel::engine::Sentinel;
use sentinel::model::*;
use sentinel::source::VortexSource;
use sentinel::store::Store;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use serde_json::{json, Value};
use tokio::sync::broadcast;
use vortex::events::logs::Invocation;
use vortex::events::{AccountRef, TxError, VortexTransaction};
use vortex::hub::HubStats;

const PROGRAM: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";

/// A feed whose chain tip the test moves (or freezes) by hand.
struct Feed {
    bus: broadcast::Sender<Arc<VortexTransaction>>,
    tip: Arc<AtomicU64>,
}

impl VortexSource for Feed {
    fn subscribe(&self) -> broadcast::Receiver<Arc<VortexTransaction>> {
        self.bus.subscribe()
    }
    fn watch_programs(&self, _: Vec<String>) {}
    fn health(&self) -> HubStats {
        HubStats {
            transactions: 0,
            last_slot: self.tip.load(Ordering::SeqCst),
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

fn sentinel(name: &str) -> (Arc<Sentinel>, Arc<Store>, Arc<AtomicU64>) {
    let db = std::env::temp_dir().join(format!("sentinel-stall-{name}-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let store = Arc::new(Store::open(db.to_str().unwrap()).unwrap());
    let (bus, _) = broadcast::channel(16);
    let tip = Arc::new(AtomicU64::new(1_000));
    let feed = Arc::new(Feed { bus, tip: tip.clone() });
    let s = Sentinel::new(store.clone(), feed, None, sentinel::pricing::PriceBook::new(), "http://ui".into()).unwrap();
    s.add_program(PROGRAM.into(), None).unwrap();
    (s, store, tip)
}

const T0: i64 = 1_700_000_000;

/// Ten minutes of steady traffic with a moving tip: a baseline, no incidents.
fn warm_up(s: &Sentinel, tip: &AtomicU64, n: &mut u64) {
    for sec in T0..T0 + 600 {
        for _ in 0..5 {
            *n += 1;
            s.on_transaction(tx(*n, sec, true));
        }
        tip.store(1_000 + (sec - T0) as u64 * 2, Ordering::SeqCst);
        s.on_tick(sec + 1);
    }
}

fn kinds(store: &Store) -> Vec<IncidentKind> {
    store.incidents(Some(PROGRAM), 20).unwrap().into_iter().map(|i| i.kind).collect()
}



#[tokio::test]
async fn a_muted_program_holds_its_notifications_and_says_so() {
    let seen: Arc<Mutex<Vec<Value>>> = Arc::default();
    let sink = seen.clone();
    let app = Router::new().route(
        "/{bot}/sendMessage",
        post(move |Path(_): Path<String>, Json(v): Json<Value>| {
            let sink = sink.clone();
            async move {
                sink.lock().unwrap().push(v);
                Json(json!({ "ok": true, "result": { "message_id": 1 } }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    std::env::set_var("SENTINEL_TELEGRAM_API", format!("http://{addr}"));

    let dir = std::env::temp_dir().join(format!("sentinel-mute-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    let store = Arc::new(Store::open(dir.to_str().unwrap()).unwrap());
    let (bus, _) = broadcast::channel(16);
    let tip = Arc::new(AtomicU64::new(1_000));
    let s = Sentinel::new(store.clone(), Arc::new(Feed { bus, tip: tip.clone() }), None, sentinel::pricing::PriceBook::new(), "http://ui".into()).unwrap();
    s.add_program(PROGRAM.into(), None).unwrap();
    store
        .create_rule(AlertRule {
            id: 0,
            owner: None,
            name: "Page on incidents".into(),
            program_id: None,
            condition: Condition::Incident { kinds: vec![], min_severity: Severity::Low },
            create_incident: false,
            severity: Severity::High,
            webhook_url: None,
            channels: vec![Channel {
                kind: ChannelKind::Telegram { bot_token: "123456789:AAEhBP0av28OdpTEST-token_value".into(), chat_id: "-100123".into() },
                min_severity: None,
            }],
            enabled: true,
            cooldown_secs: 0,
            created_at: Utc::now(),
            last_fired_at: None,
        })
        .unwrap();
    s.reload_rules().unwrap();

    // A deploy is about to start: hold notifications for an hour.
    let program = s.set_mute(PROGRAM, 60, Some("deploying v2".into())).unwrap();
    assert!(program.is_muted(Utc::now()));
    assert_eq!(program.mute_reason.as_deref(), Some("deploying v2"));

    let t0 = 1_700_000_000i64;
    let mut n = 0;
    let mut drive = |from: i64, to: i64, bad: bool| {
        for sec in from..to {
            tip.store(1_000 + sec as u64 * 3, Ordering::SeqCst);
            for i in 0..5 {
                n += 1;
                s.on_transaction(tx(n, t0 + sec, !bad || i >= 3));
            }
            s.on_tick(t0 + sec + 1);
        }
    };
    drive(0, 300, false);
    drive(300, 340, true); // the deploy goes badly
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let incidents = store.incidents(Some(PROGRAM), 10).unwrap();
    assert!(incidents.iter().any(|i| i.kind == IncidentKind::FailureSpike), "the incident is recorded");
    assert!(seen.lock().unwrap().is_empty(), "but nobody is told while it is muted");
    let log = store.executions(10).unwrap();
    assert!(log.iter().any(|e| !e.delivered && e.error.as_deref().is_some_and(|m| m.starts_with("Held: alerts for this program are muted until"))), "{log:?}");

    // Recovery inside the window: nobody heard it start, so nobody hears it end.
    drive(340, 640, false);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(seen.lock().unwrap().is_empty(), "{:?}", seen.lock().unwrap());
    assert!(store.incidents(Some(PROGRAM), 10).unwrap().iter().all(|i| i.status == IncidentStatus::Resolved));

    // Unmuted, the next incident is announced.
    s.set_mute(PROGRAM, 0, None).unwrap();
    assert!(!s.programs().is_empty());
    drive(640, 720, true);
    for _ in 0..40 {
        if !seen.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let got = seen.lock().unwrap();
    assert!(!got.is_empty(), "announced once the window is over");
    assert!(got[0]["text"].as_str().unwrap().contains("Page on incidents"));
    let _ = std::fs::remove_file(&dir);
}
