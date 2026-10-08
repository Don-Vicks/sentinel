//! The daily summary: hourly rollups feed a report that the dashboard reads and
//! the scheduler delivers once per period.

use axum::extract::Path;
use axum::{routing::post, Json, Router};
use chrono::{TimeZone, Utc};
use sentinel::engine::Sentinel;
use sentinel::model::*;
use sentinel::source::VortexSource;
use sentinel::store::Store;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast;
use vortex::events::logs::Invocation;
use vortex::events::{AccountRef, TxError, VortexTransaction};
use vortex::hub::HubStats;

const PROGRAM: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";
const BOT_TOKEN: &str = "123456789:AAEhBP0av28OdpTEST-token_value";

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


type Seen = Arc<Mutex<Vec<Value>>>;

async fn wait_for(seen: &Seen, n: usize) {
    for _ in 0..100 {
        if seen.lock().unwrap().len() >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("expected {n} deliveries, got {}", seen.lock().unwrap().len());
}

#[tokio::test]
async fn rollups_become_a_summary_that_is_delivered_once_per_period() {
    use chrono::Timelike;
    let telegram: Seen = Arc::default();
    let sink = telegram.clone();
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
    std::env::set_var("SENTINEL_ALLOW_PRIVATE_WEBHOOKS", "1");
    std::env::set_var("SENTINEL_TELEGRAM_API", format!("http://{addr}"));

    let dir = std::env::temp_dir().join(format!("sentinel-summaries-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    let store = Arc::new(Store::open(dir.to_str().unwrap()).unwrap());
    let (bus, _) = broadcast::channel(16);
    let s = Sentinel::new(store.clone(), Arc::new(FakeSource(bus)), None, sentinel::pricing::PriceBook::new(), "https://ui.example".into()).unwrap();
    s.add_program(PROGRAM.into(), None).unwrap();

    // Three hours of steady traffic ending an hour ago: 5 tx/s for a minute in each, one in ten failing.
    let now = Utc::now();
    let base = now.timestamp() - now.timestamp() % 3600 - 4 * 3600;
    let mut n = 0u64;
    for h in 0..3 {
        for sec in 0..60 {
            let at = base + h * 3600 + sec;
            for _ in 0..5 {
                n += 1;
                s.on_transaction(tx(n, at, n % 10 != 0));
            }
            s.on_tick(at + 1);
        }
    }
    let sum = s.summary(PROGRAM, 24 * 3600).unwrap();
    assert_eq!(sum.activity.tx, 900);
    assert_eq!(sum.activity.failed, 90);
    assert!((sum.activity.success_rate.unwrap() - 90.0).abs() < 1e-9);
    assert_eq!(sum.hourly.len(), 3);
    assert!(sum.hourly.iter().all(|h| h.tx == 300));
    assert!(sum.top_errors[0].label.contains("TooLittleSolReceived"), "{:?}", sum.top_errors);
    assert_eq!(sum.top_instructions[0].name, "Sell");
    let wallets = sum.activity.unique_wallets as f64;
    assert!((wallets - 50.0).abs() <= 4.0, "~50 distinct payers, got {wallets}");
    assert!(sum.activity.peak_tps >= 5.0);
    // Rollups are stored, so a restarted engine reads the same numbers.
    let again = store.rollups_between(PROGRAM, base - 3600, base + 4 * 3600).unwrap();
    assert_eq!(again.iter().map(|(_, r)| r.tx).sum::<u64>(), 900);

    // A schedule due this hour, created long ago.
    let schedule = store
        .create_schedule(SummarySchedule {
            id: 0,
            owner: "owner".into(),
            program_id: PROGRAM.into(),
            period: SummaryPeriod::Daily,
            hour_utc: now.hour() as u8,
            channels: vec![Channel {
                kind: ChannelKind::Telegram { bot_token: "123456789:AAEhBP0av28OdpTEST-token_value".into(), chat_id: "-100123".into() },
                min_severity: None,
            }],
            enabled: true,
            created_at: now - chrono::Duration::days(2),
            last_sent_at: None,
        })
        .unwrap();
    // The program must be watched by the owner for the summary to be theirs; the scheduler only
    // needs it to be monitored.
    assert_eq!(s.send_due_summaries(now), 1);
    wait_for(&telegram, 1).await;
    {
        let t = telegram.lock().unwrap();
        let text = t[0]["text"].as_str().unwrap();
        assert!(text.contains("Daily summary"), "{text}");
        assert!(text.contains("900 transactions"), "{text}");
        assert!(text.contains("90.0% succeeded"), "{text}");
        assert!(text.contains("TooLittleSolReceived"), "top error is named: {text}");
        assert!(text.contains("Sell"), "{text}");
        assert!(t[0]["reply_markup"]["inline_keyboard"][0][0]["url"].as_str().unwrap().ends_with(&format!("/programs/{PROGRAM}/summary")));
    }
    // The same period is never sent twice.
    assert_eq!(s.send_due_summaries(now + chrono::Duration::minutes(5)), 0);
    // The next day's slot is.
    assert_eq!(s.send_due_summaries(now + chrono::Duration::days(1)), 1);
    wait_for(&telegram, 2).await;
    // A schedule created after its slot waits for the next one.
    let late = store
        .create_schedule(SummarySchedule { created_at: now + chrono::Duration::days(1), last_sent_at: None, id: 0, ..schedule.clone() })
        .unwrap();
    assert_eq!(s.send_due_summaries(now + chrono::Duration::days(1) + chrono::Duration::minutes(1)), 0);
    // A report missed by more than six hours is skipped, not sent late.
    store.delete_schedule(late.id).unwrap();
    assert_eq!(s.send_due_summaries(now + chrono::Duration::days(2) + chrono::Duration::hours(7)), 0);

    // The delivery log shows the report, tied to no incident.
    let log = store.executions(10).unwrap();
    assert!(log.iter().any(|e| e.event.as_deref() == Some("summary") && e.channel.as_deref() == Some("telegram") && e.delivered && e.incident_id.is_none()));
    let _ = std::fs::remove_file(&dir);
}
