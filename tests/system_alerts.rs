//! Sentinel tells you when it can't see the chain, and when it can again.

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
async fn a_stalled_feed_is_announced_once_and_so_is_its_recovery() {
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

    let dir = std::env::temp_dir().join(format!("sentinel-system-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    let store = Arc::new(Store::open(dir.to_str().unwrap()).unwrap());
    let (bus, _) = broadcast::channel(16);
    let tip = Arc::new(AtomicU64::new(1_000));
    let s = Sentinel::new(store.clone(), Arc::new(Feed { bus, tip: tip.clone() }), None, sentinel::pricing::PriceBook::new(), "http://ui".into()).unwrap();
    s.add_program(PROGRAM.into(), None).unwrap();
    let channel = Channel {
        kind: ChannelKind::Telegram { bot_token: "123456789:AAEhBP0av28OdpTEST-token_value".into(), chat_id: "-100123".into() },
        min_severity: None,
    };
    let rule = |name: &str, kinds: Vec<SystemKind>| {
        store
            .create_rule(AlertRule {
                id: 0,
                owner: None,
                name: name.into(),
                program_id: None,
                condition: Condition::System { kinds },
                create_incident: false,
                severity: Severity::High,
                webhook_url: None,
                channels: vec![channel.clone()],
                enabled: true,
                cooldown_secs: 0,
                created_at: Utc::now(),
                last_fired_at: None,
            })
            .unwrap()
    };
    rule("Sentinel is blind", vec![]);
    rule("RPC only", vec![SystemKind::RpcFailing]);
    s.reload_rules().unwrap();

    // A moving feed says nothing.
    let t0 = 1_700_000_000i64;
    for sec in 0..30 {
        tip.store(1_000 + sec as u64 * 3, Ordering::SeqCst);
        s.on_tick(t0 + sec);
    }
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(seen.lock().unwrap().is_empty());

    // The tip freezes: one announcement, however long it lasts.
    for sec in 30..120 {
        s.on_tick(t0 + sec);
    }
    for _ in 0..40 {
        if !seen.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    {
        let got = seen.lock().unwrap();
        assert_eq!(got.len(), 1, "announced once: {got:?}");
        let text = got[0]["text"].as_str().unwrap();
        assert!(text.contains("Sentinel is blind") && text.contains("feed has stalled"), "{text}");
        assert!(text.contains("detectors are paused"), "{text}");
    }

    // It moves again: one recovery message, and only for the rule that asked about the feed.
    for sec in 120..150 {
        tip.store(2_000 + sec as u64 * 3, Ordering::SeqCst);
        s.on_tick(t0 + sec);
    }
    for _ in 0..40 {
        if seen.lock().unwrap().len() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let got = seen.lock().unwrap();
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(got[1]["text"].as_str().unwrap().contains("moving again"), "{}", got[1]["text"]);
    let log = store.executions(10).unwrap();
    assert!(log.iter().all(|e| e.rule_name == "Sentinel is blind"), "the RPC-only rule stayed quiet");
    assert!(log.iter().any(|e| e.event.as_deref() == Some("resolved") && e.delivered));
    let _ = std::fs::remove_file(&dir);
}
