//! Alerts follow the incident across channels: Telegram replies to the first
//! message, PagerDuty resolves what it triggered, Slack gets Block Kit.

use axum::extract::Path;
use axum::{routing::post, Json, Router};
use chrono::{TimeZone, Utc};
use sentinel::engine::Sentinel;
use sentinel::model::*;
use sentinel::source::VortexSource;
use sentinel::store::Store;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast;
use vortex::events::logs::Invocation;
use vortex::events::{AccountRef, TxError, VortexTransaction};
use vortex::hub::HubStats;

const PROGRAM: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";
const BOT_TOKEN: &str = "123456789:AAEhBP0av28OdpTEST-token_value";
const ROUTING_KEY: &str = "R0UT1NGKEYFORTESTSONLY0123456789";

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
async fn alerts_follow_the_incident_across_channels() {
    let telegram: Seen = Arc::default();
    let pagerduty: Seen = Arc::default();
    let slack: Seen = Arc::default();
    let paths: Arc<Mutex<Vec<String>>> = Arc::default();
    let next_id = Arc::new(AtomicI64::new(500));

    let (t_sink, t_paths, t_id) = (telegram.clone(), paths.clone(), next_id.clone());
    let (p_sink, s_sink) = (pagerduty.clone(), slack.clone());
    let app = Router::new()
        .route(
            "/{bot}/sendMessage",
            post(move |Path(bot): Path<String>, Json(v): Json<Value>| {
                let (sink, paths, id) = (t_sink.clone(), t_paths.clone(), t_id.clone());
                async move {
                    paths.lock().unwrap().push(bot);
                    sink.lock().unwrap().push(v);
                    Json(json!({ "ok": true, "result": { "message_id": id.fetch_add(1, Ordering::SeqCst) } }))
                }
            }),
        )
        .route(
            "/pd",
            post(move |Json(v): Json<Value>| {
                let sink = p_sink.clone();
                async move {
                    sink.lock().unwrap().push(v);
                    (axum::http::StatusCode::ACCEPTED, Json(json!({ "status": "success" })))
                }
            }),
        )
        .route(
            "/slack",
            post(move |Json(v): Json<Value>| {
                let sink = s_sink.clone();
                async move {
                    sink.lock().unwrap().push(v);
                    "ok"
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    std::env::set_var("SENTINEL_ALLOW_PRIVATE_WEBHOOKS", "1");
    std::env::set_var("SENTINEL_TELEGRAM_API", format!("http://{addr}"));
    std::env::set_var("SENTINEL_PAGERDUTY_API", format!("http://{addr}/pd"));

    let dir = std::env::temp_dir().join(format!("sentinel-channels-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    let store = Arc::new(Store::open(dir.to_str().unwrap()).unwrap());
    let (bus, _) = broadcast::channel(16);
    let s = Sentinel::new(store.clone(), Arc::new(FakeSource(bus)), None, sentinel::pricing::PriceBook::new(), "http://ui".into()).unwrap();
    s.add_program(PROGRAM.into(), None).unwrap();
    store
        .create_rule(AlertRule {
            id: 0,
            owner: None,
            name: "Page on incidents".into(),
            program_id: None,
            condition: Condition::Incident { kinds: vec![], min_severity: Severity::Medium },
            create_incident: false,
            severity: Severity::High,
            webhook_url: None,
            channels: vec![
                Channel {
                    kind: ChannelKind::Telegram { bot_token: BOT_TOKEN.into(), chat_id: "-1001234567890".into() },
                    min_severity: None,
                },
                Channel { kind: ChannelKind::Pagerduty { routing_key: ROUTING_KEY.into() }, min_severity: None },
                Channel { kind: ChannelKind::Slack { url: format!("http://{addr}/slack") }, min_severity: None },
            ],
            enabled: true,
            cooldown_secs: 0,
            created_at: Utc::now(),
            last_fired_at: None,
        })
        .unwrap();
    s.reload_rules().unwrap();

    let t0 = 1_700_000_000i64;
    let mut n = 0;
    for sec in t0..t0 + 300 {
        for _ in 0..5 {
            n += 1;
            s.on_transaction(tx(n, sec, n % 50 != 0));
        }
        s.on_tick(sec + 1);
    }
    for sec in t0 + 300..t0 + 330 {
        for i in 0..5 {
            n += 1;
            s.on_transaction(tx(n, sec, i >= 3));
        }
        s.on_tick(sec + 1);
    }
    let incident = store
        .incidents(Some(PROGRAM), 10)
        .unwrap()
        .into_iter()
        .find(|i| i.kind == IncidentKind::FailureSpike)
        .expect("failure spike incident");

    // The incident opens: one message per channel, none of them replies yet.
    wait_for(&telegram, 1).await;
    wait_for(&pagerduty, 1).await;
    wait_for(&slack, 1).await;
    {
        let t = telegram.lock().unwrap();
        let first = &t[0];
        assert_eq!(first["chat_id"], "-1001234567890");
        assert_eq!(first["parse_mode"], "HTML");
        assert!(first.get("reply_to_message_id").is_none());
        let text = first["text"].as_str().unwrap();
        assert!(text.contains("Page on incidents"), "{text}");
        assert!(text.contains("TooLittleSolReceived"), "root cause is named: {text}");
        // The bot token is part of the request path only, never the message.
        assert!(!text.contains(BOT_TOKEN));
        assert_eq!(paths.lock().unwrap()[0], format!("bot{BOT_TOKEN}"));

        let pd = pagerduty.lock().unwrap();
        assert_eq!(pd[0]["event_action"], "trigger");
        assert_eq!(pd[0]["routing_key"], ROUTING_KEY);
        assert_eq!(pd[0]["dedup_key"], format!("sentinel-incident-{}", incident.id));
        assert!(matches!(pd[0]["payload"]["severity"].as_str(), Some("critical" | "error" | "warning")));

        let sl = slack.lock().unwrap();
        let blocks = sl[0]["attachments"][0]["blocks"].as_array().expect("Block Kit blocks");
        assert!(blocks.iter().any(|b| b["type"] == "header"));
        assert!(blocks.iter().any(|b| b["type"] == "actions"), "investigate button");
    }

    // In production the resolution comes minutes later; wait until the opening
    // deliveries are fully recorded (which is when their message ids are kept).
    for _ in 0..100 {
        if store.executions(50).unwrap().iter().filter(|e| e.event.as_deref() == Some("opened") && e.delivered).count() >= 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Recovery resolves it: Telegram replies to the first message, PagerDuty resolves.
    for sec in t0 + 330..t0 + 600 {
        for _ in 0..5 {
            n += 1;
            s.on_transaction(tx(n, sec, true));
        }
        s.on_tick(sec + 1);
    }
    let resolved = store.incident(incident.id).unwrap().unwrap();
    assert_eq!(resolved.status, IncidentStatus::Resolved);
    // The failure rate also escalated from Medium to High on the way, so each
    // channel saw: opened, escalated, resolved.
    wait_for(&telegram, 3).await;
    wait_for(&pagerduty, 3).await;
    wait_for(&slack, 3).await;
    let t = telegram.lock().unwrap();
    assert!(t[1]["text"].as_str().unwrap().contains("Escalated"));
    assert_eq!(t[1]["reply_to_message_id"], 500, "the escalation replies to the opening message");
    assert!(t[2]["text"].as_str().unwrap().contains("Resolved"));
    assert_eq!(t[2]["reply_to_message_id"], 500, "and so does the resolution");
    let pd = pagerduty.lock().unwrap();
    assert_eq!(pd[1]["event_action"], "trigger", "escalation updates the same alert");
    assert_eq!(pd[1]["payload"]["severity"], "error");
    assert_eq!(pd[1]["dedup_key"], pd[0]["dedup_key"]);
    assert_eq!(pd[2]["event_action"], "resolve");
    assert_eq!(pd[2]["dedup_key"], pd[0]["dedup_key"], "resolves what it triggered");

    // The delivery log says which channel and which event.
    let log = store.executions(50).unwrap();
    for channel in ["telegram", "pagerduty", "slack"] {
        for event in ["opened", "updated", "resolved"] {
            assert!(
                log.iter().any(|e| e.channel.as_deref() == Some(channel) && e.event.as_deref() == Some(event) && e.delivered),
                "{channel} {event} delivered"
            );
        }
    }
    // Secrets never reach the log.
    let dump = serde_json::to_string(&log).unwrap();
    assert!(!dump.contains(BOT_TOKEN) && !dump.contains(ROUTING_KEY), "{dump}");
    let _ = std::fs::remove_file(&dir);
}
