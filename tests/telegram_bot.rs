//! The Telegram bot: commands from the chats that receive alerts, and the Acknowledge button.

use chrono::{TimeZone, Utc};
use sentinel::engine::Sentinel;
use sentinel::model::*;
use axum::extract::Path;
use axum::{routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::Mutex;
use sentinel::source::VortexSource;
use sentinel::store::Store;
use std::sync::Arc;
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


const TOKEN: &str = "123456789:AAEhBP0av28OdpTEST-token_value";
const CHAT: i64 = -1001234567890;

#[derive(Default)]
struct Telegram {
    /// Updates to hand out on the next getUpdates.
    queue: Mutex<Vec<Value>>,
    sent: Mutex<Vec<(String, Value)>>,
}

async fn mock() -> (String, Arc<Telegram>) {
    let tg = Arc::new(Telegram::default());
    let t = tg.clone();
    let app = Router::new().route(
        "/{bot}/{method}",
        post(move |Path((_, method)): Path<(String, String)>, Json(body): Json<Value>| {
            let t = t.clone();
            async move {
                if method == "getUpdates" {
                    let batch: Vec<Value> = std::mem::take(&mut *t.queue.lock().unwrap());
                    return Json(json!({ "ok": true, "result": batch }));
                }
                t.sent.lock().unwrap().push((method, body));
                Json(json!({ "ok": true, "result": { "message_id": 1 } }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), tg)
}

fn message(id: i64, chat: i64, text: &str) -> Value {
    json!({ "update_id": id, "message": { "message_id": id, "chat": { "id": chat }, "from": { "first_name": "Ana" }, "text": text } })
}

fn incident(store: &Store, program: &str, title: &str) -> Incident {
    store
        .create_incident(
            serde_json::from_value(json!({
                "id": 0, "program_id": program, "kind": "failure_spike", "severity": "high", "status": "open",
                "title": title, "summary": "45% of transactions failing", "explanation": "x", "source": "detector",
                "metric": null, "observed": null, "peak": null, "baseline": null, "threshold": null, "onset_at": null,
                "detected_at": "2024-01-01T00:00:00Z", "updated_at": "2024-01-01T00:00:00Z", "resolved_at": null,
                "detection_latency_ms": null, "affected_count": 3, "affected_wallets": 2, "evidence": {}
            }))
            .unwrap(),
        )
        .unwrap()
}

#[tokio::test]
async fn the_chat_that_gets_alerts_can_ask_questions_and_acknowledge() {
    let (url, tg) = mock().await;
    std::env::set_var("SENTINEL_TELEGRAM_API", &url);
    let dir = std::env::temp_dir().join(format!("sentinel-bot-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    let store = Arc::new(Store::open(dir.to_str().unwrap()).unwrap());
    let (bus, _) = broadcast::channel(16);
    let s = Sentinel::new(store.clone(), Arc::new(FakeSource(bus)), None, sentinel::pricing::PriceBook::new(), "https://ui.example".into()).unwrap();
    // Ana watches Pump.fun; Bo watches something else and must stay out of it.
    s.watch("ana", PROGRAM.into(), None).unwrap();
    s.watch("bo", "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4".into(), None).unwrap();
    store
        .create_rule(AlertRule {
            id: 0,
            owner: Some("ana".into()),
            name: "Page on incidents".into(),
            program_id: None,
            condition: Condition::Incident { kinds: vec![], min_severity: Severity::Low },
            create_incident: false,
            severity: Severity::High,
            webhook_url: None,
            channels: vec![Channel { kind: ChannelKind::Telegram { bot_token: TOKEN.into(), chat_id: CHAT.to_string() }, min_severity: None }],
            enabled: true,
            cooldown_secs: 0,
            created_at: Utc::now(),
            last_fired_at: None,
        })
        .unwrap();
    s.reload_rules().unwrap();
    let mine = incident(&store, PROGRAM, "Transaction failure spike · Pump.fun");
    let theirs = incident(&store, "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4", "Failure spike · Jupiter");
    let client = reqwest::Client::new();
    let ask = |updates: Vec<Value>| {
        let (s, tg, client) = (s.clone(), tg.clone(), client.clone());
        async move {
            tg.sent.lock().unwrap().clear();
            *tg.queue.lock().unwrap() = updates;
            let next = sentinel::telegram::poll_once(&s, &client, TOKEN, 0, 0).await.unwrap();
            let sent: Vec<(String, Value)> = tg.sent.lock().unwrap().clone();
            (next, sent)
        }
    };
    let texts = |sent: &[(String, Value)]| sent.iter().filter(|(m, _)| m == "sendMessage").map(|(_, b)| b["text"].as_str().unwrap().to_string()).collect::<Vec<_>>();

    // A stranger's chat gets no answer at all, not even an error.
    let (next, sent) = ask(vec![message(10, 999, "/status")]).await;
    assert!(sent.is_empty(), "{sent:?}");
    assert_eq!(next, 11, "but the update is consumed");

    // /status and /incidents answer for Ana's programs only.
    let (_, sent) = ask(vec![message(11, CHAT, "/status"), message(12, CHAT, "/incidents")]).await;
    let t = texts(&sent);
    assert!(t[0].contains("Pump.fun") && !t[0].contains("Jupiter"), "{}", t[0]);
    assert!(t[1].contains(&format!("#{}", mine.id)) && !t[1].contains("Jupiter"), "{}", t[1]);
    assert_eq!(sent[0].1["chat_id"], CHAT);
    assert_eq!(sent[0].1["parse_mode"], "HTML");

    // /health and /summary work without naming the program, as there is only one.
    let (_, sent) = ask(vec![message(13, CHAT, "/health"), message(14, CHAT, "/summary 7d")]).await;
    let t = texts(&sent);
    assert!(t[0].contains("Pump.fun"), "{}", t[0]);
    assert!(t[1].contains("Pump.fun") && t[1].contains("7 days"), "{}", t[1]);

    // /mute holds notifications; /unmute lifts it.
    let (_, sent) = ask(vec![message(15, CHAT, "/mute 45m")]).await;
    assert!(texts(&sent)[0].contains("muted for 45 minutes"), "{:?}", texts(&sent));
    assert!(s.program_detail(PROGRAM).unwrap()["program"]["muted_until"].is_string());
    let (_, sent) = ask(vec![message(16, CHAT, "/unmute pump.fun")]).await;
    assert!(texts(&sent)[0].contains("live again"));
    assert!(s.program_detail(PROGRAM).unwrap()["program"]["muted_until"].is_null());
    let (_, sent) = ask(vec![message(17, CHAT, "/mute soon")]).await;
    assert!(texts(&sent)[0].contains("how long"), "{:?}", texts(&sent));

    // /ack acts on Ana's incident, and refuses Bo's.
    let (_, sent) = ask(vec![message(18, CHAT, &format!("/ack {}", mine.id)), message(19, CHAT, &format!("/ack {}", theirs.id))]).await;
    let t = texts(&sent);
    assert!(t[0].contains("being investigated"), "{}", t[0]);
    assert!(t[1].contains("can't find"), "{}", t[1]);
    assert_eq!(store.incident(mine.id).unwrap().unwrap().status, IncidentStatus::Investigating);
    assert_eq!(store.incident(theirs.id).unwrap().unwrap().status, IncidentStatus::Open, "someone else's incident is untouched");

    // The Acknowledge button on an alert does the same, and says who tapped it.
    let second = incident(&store, PROGRAM, "Error spike · Pump.fun");
    let tap = json!({ "update_id": 20, "callback_query": { "id": "cb1", "data": format!("ack:{}", second.id), "from": { "first_name": "Ana" },
        "message": { "message_id": 7, "chat": { "id": CHAT } } } });
    let (_, sent) = ask(vec![tap]).await;
    assert!(sent.iter().any(|(m, b)| m == "answerCallbackQuery" && b["callback_query_id"] == "cb1"), "{sent:?}");
    assert!(texts(&sent)[0].contains("Ana is investigating incident"), "{:?}", texts(&sent));
    assert_eq!(store.incident(second.id).unwrap().unwrap().status, IncidentStatus::Investigating);
    // A tap from a chat that isn't Ana's does nothing.
    let stray = json!({ "update_id": 21, "callback_query": { "id": "cb2", "data": format!("ack:{}", theirs.id), "from": { "first_name": "X" },
        "message": { "message_id": 8, "chat": { "id": 31337 } } } });
    let (_, sent) = ask(vec![stray]).await;
    assert!(sent.is_empty());
    let _ = std::fs::remove_file(&dir);
}
