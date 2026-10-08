//! Alert rules on decoded instructions, against Pump.fun's real on-chain IDL and
//! real mainnet transactions.

use chrono::Utc;
use sentinel::engine::Sentinel;
use sentinel::idl::{decode_idl_account, Idl};
use sentinel::model::*;
use sentinel::source::VortexSource;
use sentinel::store::Store;
use serde_json::{json, Value};
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

fn instruction(name: &str, filters: Vec<ArgFilter>, first_seen_signer: bool) -> Condition {
    Condition::Instruction {
        name: name.into(),
        program_id: None,
        filters,
        match_mode: MatchMode::All,
        success_only: true,
        first_seen_signer,
    }
}

fn filter(path: &str, op: FilterOp, value: Value) -> ArgFilter {
    ArgFilter { path: path.into(), op, value }
}

fn rule_incidents(store: &Store, rule_id: i64) -> Vec<Incident> {
    store.incidents(None, 50).unwrap().into_iter().filter(|i| i.source == format!("rule:{rule_id}")).collect()
}

#[tokio::test]
async fn rules_fire_on_decoded_names_arguments_and_accounts() {
    let dir = std::env::temp_dir().join(format!("sentinel-ixrules-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    let store = Arc::new(Store::open(dir.to_str().unwrap()).unwrap());
    let (bus, _) = broadcast::channel(16);
    let s = Sentinel::new(store.clone(), Arc::new(FakeSource(bus)), None, sentinel::pricing::PriceBook::new(), "http://ui".into()).unwrap();
    s.add_program(PUMP.into(), None).unwrap();
    s.idls.insert(pump_idl());

    let buy = fixture("pump_ok");
    let signer = buy.fee_payer().unwrap().to_string();
    let ix = buy.instructions.iter().find(|i| i.program_id == PUMP && i.name.as_deref() == Some("Buy")).unwrap();
    let data = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &ix.data).unwrap();
    let decoded = pump_idl().decode_instruction(&data, &ix.accounts).unwrap();
    let amount = decoded.args["amount"].as_u64().unwrap();
    let user = decoded.accounts.iter().find(|a| a.name == "user").unwrap().pubkey.clone();

    let by_name = rule(&store, "Any buy", instruction("buy", vec![], false));
    let big = rule(&store, "Big buy", instruction("Buy", vec![filter("args.amount", FilterOp::Gt, json!(amount - 1))], false));
    let too_big = rule(&store, "Huge buy", instruction("buy", vec![filter("args.amount", FilterOp::Gt, json!(amount))], false));
    let by_account = rule(&store, "Buy by user", instruction("buy", vec![filter("accounts.user", FilterOp::Eq, json!(user))], false));
    let other_name = rule(&store, "Sells", instruction("sell", vec![], false));
    let pattern = rule(&store, "Buy or sell", instruction("buy|sell", vec![], false));
    s.reload_rules().unwrap();

    s.on_transaction(Arc::new(buy.clone()));
    s.flush();

    assert_eq!(rule_incidents(&store, by_name.id).len(), 1, "matches by name");
    assert_eq!(rule_incidents(&store, big.id).len(), 1, "amount {amount} is above {}", amount - 1);
    assert!(rule_incidents(&store, too_big.id).is_empty(), "amount is not above itself");
    assert_eq!(rule_incidents(&store, by_account.id).len(), 1, "matches a named account");
    assert!(rule_incidents(&store, other_name.id).is_empty(), "a sell rule ignores a buy");
    assert_eq!(rule_incidents(&store, pattern.id).len(), 1, "patterns separated by |");

    let incident = &rule_incidents(&store, big.id)[0];
    assert!(incident.summary.contains("buy called by"), "{}", incident.summary);
    assert!(incident.summary.contains(&format!("amount = {amount}")), "says what it saw: {}", incident.summary);
    assert_eq!(incident.observed, Some(amount as f64));
    assert_eq!(store.incident_transactions(incident.id, 5).unwrap().len(), 1, "the transaction is linked");
    let _ = signer;

    // A failed call changed nothing, so it doesn't count.
    let failed = fixture("pump_failed");
    s.on_transaction(Arc::new(failed));
    assert_eq!(rule_incidents(&store, by_name.id).len(), 1);
    let _ = std::fs::remove_file(&dir);
}

#[tokio::test]
async fn a_rule_about_arguments_waits_for_the_idl_but_a_rule_about_names_does_not() {
    let dir = std::env::temp_dir().join(format!("sentinel-ixrules-noidl-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    let store = Arc::new(Store::open(dir.to_str().unwrap()).unwrap());
    let (bus, _) = broadcast::channel(16);
    let s = Sentinel::new(store.clone(), Arc::new(FakeSource(bus)), None, sentinel::pricing::PriceBook::new(), "http://ui".into()).unwrap();
    s.add_program(PUMP.into(), None).unwrap();
    // No IDL is loaded. Vortex still names the instruction from the program's logs.
    let by_name = rule(&store, "Buy", instruction("Buy", vec![], false));
    let by_args = rule(&store, "Big buy", instruction("buy", vec![filter("args.amount", FilterOp::Gt, json!(0))], false));
    s.reload_rules().unwrap();
    s.on_transaction(Arc::new(fixture("pump_ok")));
    assert_eq!(rule_incidents(&store, by_name.id).len(), 1);
    assert!(rule_incidents(&store, by_args.id).is_empty(), "arguments can't be judged without the IDL");
    let _ = std::fs::remove_file(&dir);
}

#[tokio::test]
async fn first_time_signers_are_flagged_once_after_the_program_has_been_learned() {
    let dir = std::env::temp_dir().join(format!("sentinel-ixrules-first-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    let store = Arc::new(Store::open(dir.to_str().unwrap()).unwrap());
    let (bus, _) = broadcast::channel(16);
    let s = Sentinel::new(store.clone(), Arc::new(FakeSource(bus)), None, sentinel::pricing::PriceBook::new(), "http://ui".into()).unwrap();
    s.add_program(PUMP.into(), None).unwrap();
    s.idls.insert(pump_idl());
    let new_signer = rule(&store, "Admin from a new wallet", instruction("buy", vec![], true));
    s.reload_rules().unwrap();

    let mut tx = fixture("pump_ok");
    // Straight after start-up every wallet looks new, so nothing is flagged until the program is learned.
    s.on_transaction(Arc::new(tx.clone()));
    assert!(rule_incidents(&store, new_signer.id).is_empty(), "still learning who the regulars are");

    let mut cfg = DetectionConfig::default();
    cfg.warmup_secs = 0;
    s.update_program(PUMP, None, Some(cfg)).unwrap();

    // The wallet from before is known now. A different wallet is not.
    tx.signature = "second-signature-from-the-known-wallet".into();
    s.on_transaction(Arc::new(tx.clone()));
    assert!(rule_incidents(&store, new_signer.id).is_empty(), "known wallet");

    let mut stranger = tx.clone();
    stranger.signature = "third-signature-from-a-stranger".into();
    stranger.accounts[0].pubkey = "Stranger1111111111111111111111111111111111".into();
    for ix in stranger.instructions.iter_mut() {
        for a in ix.accounts.iter_mut() {
            if *a == tx.accounts[0].pubkey {
                *a = "Stranger1111111111111111111111111111111111".into();
            }
        }
    }
    s.on_transaction(Arc::new(stranger.clone()));
    let flagged = rule_incidents(&store, new_signer.id);
    assert_eq!(flagged.len(), 1, "the stranger is flagged");
    assert!(flagged[0].summary.contains("the first time this wallet has called it"), "{}", flagged[0].summary);

    // Once is enough: the same stranger again, even after a restart's worth of memory loss, is known.
    stranger.signature = "fourth".into();
    s.on_transaction(Arc::new(stranger));
    assert_eq!(rule_incidents(&store, new_signer.id).len(), 1);
    assert!(!store.mark_signer_seen(PUMP, "buy", "Stranger1111111111111111111111111111111111").unwrap(), "remembered in the database");
    let _ = std::fs::remove_file(&dir);
}
