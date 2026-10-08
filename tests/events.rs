//! Program events (`emit!` and `emit_cpi!`), decoded with Pump.fun's real on-chain IDL from a
//! real mainnet transaction.

use chrono::Utc;
use sentinel::engine::Sentinel;
use sentinel::idl::{decode_idl_account, Idl};
use sentinel::model::*;
use sentinel::source::VortexSource;
use sentinel::store::Store;
use serde_json::{json, Value};
use solana_sdk::pubkey::Pubkey;
use std::sync::Arc;
use tokio::sync::broadcast;
use vortex::events::VortexTransaction;
use vortex::geyser::decode::decode_transaction;
use vortex::geyser::rpc_frame::frame_from_rpc_json;
use vortex::hub::HubStats;

const PUMP: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";

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

fn fixture(name: &str) -> VortexTransaction {
    let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let f: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let resp = &f["response"];
    let frame = frame_from_rpc_json(f["signature"].as_str().unwrap(), resp["slot"].as_u64().unwrap(), resp).unwrap();
    decode_transaction(frame, vec![]).unwrap()
}

fn pump_idl() -> Idl {
    let data = std::fs::read(format!("{}/tests/fixtures/pump_idl_account.bin", env!("CARGO_MANIFEST_DIR"))).unwrap();
    Idl::parse(PUMP, &decode_idl_account(&data).unwrap()).unwrap()
}

fn rule(store: &Store, name: &str, condition: Condition) -> AlertRule {
    store
        .create_rule(AlertRule {
            id: 0,
            owner: None,
            name: name.into(),
            program_id: None,
            condition,
            create_incident: true,
            severity: Severity::High,
            webhook_url: None,
            channels: vec![],
            enabled: true,
            cooldown_secs: 0,
            created_at: Utc::now(),
            last_fired_at: None,
        })
        .unwrap()
}

fn rule_incidents(store: &Store, rule_id: i64) -> Vec<Incident> {
    store.incidents(None, 50).unwrap().into_iter().filter(|i| i.source == format!("rule:{rule_id}")).collect()
}

fn event(name: &str, filters: Vec<ArgFilter>) -> Condition {
    Condition::Event { name: name.into(), program_id: None, filters, match_mode: MatchMode::All, success_only: true }
}

fn filter(path: &str, op: FilterOp, value: Value) -> ArgFilter {
    ArgFilter { path: path.into(), op, value }
}

fn sentinel(tag: &str) -> (Arc<Store>, Arc<Sentinel>, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("sentinel-events-{tag}-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    let store = Arc::new(Store::open(dir.to_str().unwrap()).unwrap());
    let (bus, _) = broadcast::channel(16);
    let s = Sentinel::new(store.clone(), Arc::new(FakeSource(bus)), None, sentinel::pricing::PriceBook::new(), "http://ui".into()).unwrap();
    s.add_program(PUMP.into(), None).unwrap();
    (store, s, dir)
}

#[test]
fn an_event_logged_and_emitted_by_cpi_is_one_event() {
    let tx = fixture("pump_ok");
    let idl = pump_idl();
    assert!(sentinel::events::may_carry(&tx));
    let evs = sentinel::events::emitted(&tx, PUMP, &idl);
    assert_eq!(evs.len(), 1, "Pump.fun writes its TradeEvent to the log and to a self-CPI: {evs:?}");
    assert_eq!(evs[0].name, "TradeEvent");
    assert_eq!(evs[0].fields["is_buy"], true);
    assert_eq!(evs[0].fields["sol_amount"], 60623340);
    assert_eq!(evs[0].fields["user"], "BwWK17cbHxwWBKZkUYvzxLcNQ1YVyaFezduWbtm2de6s");
    // A program that isn't the emitter has no events here.
    assert!(sentinel::events::emitted(&tx, "MAyhSmzXzV1pTf7LsNkrNwkWKTo4ougAJ1PPg47MD4e", &idl).is_empty());
    let schema = idl.event_schema();
    let trade = schema.iter().find(|e| e.name == "TradeEvent").unwrap();
    assert!(trade.fields.iter().any(|f| f.path == "fields.sol_amount" && f.r#type == "u64"));
}

#[tokio::test]
async fn event_rules_fire_on_decoded_names_and_fields() {
    let (store, s, dir) = sentinel("rules");
    s.idls.insert(pump_idl());

    let any = rule(&store, "Any trade", event("TradeEvent", vec![]));
    let big = rule(&store, "Big trade", event("tradeevent", vec![filter("fields.sol_amount", FilterOp::Gt, json!(60_000_000))]));
    let huge = rule(&store, "Huge trade", event("TradeEvent", vec![filter("fields.sol_amount", FilterOp::Gt, json!(60_623_340))]));
    let buys = rule(&store, "Buys", event("TradeEvent", vec![filter("fields.is_buy", FilterOp::Eq, json!(true))]));
    let sells = rule(&store, "Sells", event("TradeEvent", vec![filter("fields.is_buy", FilterOp::Eq, json!(false))]));
    let by_user = rule(&store, "One wallet", event("*", vec![filter("fields.user", FilterOp::Eq, json!("BwWK17cbHxwWBKZkUYvzxLcNQ1YVyaFezduWbtm2de6s"))]));
    let has_creator = rule(&store, "Has creator", event("TradeEvent", vec![filter("fields.creator", FilterOp::Exists, json!(null))]));
    let no_such = rule(&store, "Missing field", event("TradeEvent", vec![filter("fields.nope", FilterOp::Exists, json!(null))]));
    let other = rule(&store, "Created", event("CreateEvent", vec![]));
    s.reload_rules().unwrap();

    s.on_transaction(Arc::new(fixture("pump_ok")));
    s.flush();

    assert_eq!(rule_incidents(&store, any.id).len(), 1, "one trade, one incident, though it was logged twice");
    assert_eq!(rule_incidents(&store, big.id).len(), 1, "names ignore case; numbers compare");
    assert!(rule_incidents(&store, huge.id).is_empty());
    assert_eq!(rule_incidents(&store, buys.id).len(), 1);
    assert!(rule_incidents(&store, sells.id).is_empty());
    assert_eq!(rule_incidents(&store, by_user.id).len(), 1, "a pattern with a field filter");
    assert_eq!(rule_incidents(&store, has_creator.id).len(), 1, "exists");
    assert!(rule_incidents(&store, no_such.id).is_empty(), "exists on a field that isn't there");
    assert!(rule_incidents(&store, other.id).is_empty());

    let incident = &rule_incidents(&store, big.id)[0];
    assert!(incident.summary.starts_with("TradeEvent emitted in"), "{}", incident.summary);
    assert!(incident.summary.contains("sol_amount = 60623340"), "{}", incident.summary);
    assert_eq!(incident.observed, Some(60623340.0));

    // The dashboard keeps what was decoded.
    s.flush();
    let recent = s.recent_events(PUMP, None, 10);
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].name, "TradeEvent");

    // A failed transaction emitted nothing that stuck.
    s.on_transaction(Arc::new(fixture("pump_failed")));
    assert_eq!(rule_incidents(&store, any.id).len(), 1);
    assert_eq!(s.recent_events(PUMP, None, 10).len(), 1);
    let _ = std::fs::remove_file(&dir);
}

#[tokio::test]
async fn without_the_idl_event_rules_wait_instead_of_guessing() {
    let (store, s, dir) = sentinel("noidl");
    let any = rule(&store, "Any trade", event("TradeEvent", vec![]));
    s.reload_rules().unwrap();
    s.on_transaction(Arc::new(fixture("pump_ok")));
    s.flush();
    assert!(rule_incidents(&store, any.id).is_empty(), "event names come from the IDL");
    s.flush();
    assert!(s.recent_events(PUMP, None, 10).is_empty());
    let _ = std::fs::remove_file(&dir);
}

#[tokio::test]
async fn a_hand_written_schema_decodes_events_of_a_program_that_is_not_anchor() {
    // A native program that logs `[tag u8][amount u64][user pubkey]` with sol_log_data, and has no IDL.
    let schema = json!({
        "events": [
            { "name": "Deposit", "discriminator": [7], "fields": [{ "name": "amount", "type": "u64" }, { "name": "user", "type": "pubkey" }] },
            { "name": "Halted", "discriminator": [9], "fields": [] }
        ]
    });
    let user = Pubkey::new_from_array([3; 32]);
    let log_event = |tag: u8, amount: Option<u64>| {
        let mut data = vec![tag];
        if let Some(a) = amount {
            data.extend(a.to_le_bytes());
            data.extend(user.to_bytes());
        }
        format!("Program data: {}", base64::Engine::encode(&base64::engine::general_purpose::STANDARD, data))
    };
    let mut tx = fixture("pump_ok");
    tx.instructions.clear();
    tx.logs = vec![
        format!("Program {PUMP} invoke [1]"),
        log_event(7, Some(2_500_000_000)),
        log_event(9, None),
        log_event(1, None), // a tag the schema doesn't name
        format!("Program {PUMP} success"),
    ];

    let (store, s, dir) = sentinel("schema");
    let big = rule(&store, "Big deposit", event("Deposit", vec![filter("fields.amount", FilterOp::Gt, json!(1_000_000_000u64))]));
    let halted = rule(&store, "Halted", event("Halted", vec![]));
    let small = rule(&store, "Small deposit", event("Deposit", vec![filter("fields.amount", FilterOp::Lt, json!(1000))]));
    s.reload_rules().unwrap();

    // Not an IDL at all, and not a schema: refused with a reason.
    assert!(s.set_custom_idl(PUMP, "me", &json!({ "hello": "world" })).is_err());
    let summary = s.set_custom_idl(PUMP, "me", &schema).unwrap();
    assert_eq!((summary["instructions"].as_u64(), summary["events"].as_u64()), (Some(0), Some(2)));

    let got = sentinel::events::emitted(&tx, PUMP, &s.idls.cached(PUMP).unwrap());
    assert_eq!(got.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), vec!["Deposit", "Halted"], "{got:?}");
    assert_eq!(got[0].fields["amount"], 2_500_000_000u64);
    assert_eq!(got[0].fields["user"], user.to_string());

    s.on_transaction(Arc::new(tx));
    s.flush();
    assert_eq!(rule_incidents(&store, big.id).len(), 1);
    assert_eq!(rule_incidents(&store, halted.id).len(), 1);
    assert!(rule_incidents(&store, small.id).is_empty());
    assert_eq!(s.recent_events(PUMP, None, 10).len(), 2);
    let _ = std::fs::remove_file(&dir);
}
