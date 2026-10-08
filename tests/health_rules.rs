//! Rules on the health score and on a wallet's SOL balance, and the health history kept for summaries.

use chrono::{TimeZone, Utc};
use sentinel::engine::Sentinel;
use sentinel::model::*;
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

#[tokio::test]
async fn a_health_rule_fires_when_the_score_drops_and_the_score_is_remembered() {
    let dir = std::env::temp_dir().join(format!("sentinel-healthrules-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    let store = Arc::new(Store::open(dir.to_str().unwrap()).unwrap());
    let (bus, _) = broadcast::channel(16);
    let s = Sentinel::new(store.clone(), Arc::new(FakeSource(bus)), None, sentinel::pricing::PriceBook::new(), "http://ui".into()).unwrap();
    s.add_program(PROGRAM.into(), None).unwrap();
    let health = rule(&store, "Health below 70", Condition::Health { below: 70 });
    let keeper = format!("Payer{:0>39}", 7);
    let wallet = rule(&store, "Keeper low on SOL", Condition::WalletBalance { account: keeper.clone(), below_sol: 1.0 });
    s.reload_rules().unwrap();

    let t0 = 1_700_000_100i64;
    let mut n = 0;
    for sec in 0..360 {
        for _ in 0..5 {
            n += 1;
            s.on_transaction(traffic(n, t0 + sec, n % 50 != 0));
        }
        s.on_tick(t0 + sec + 1);
    }
    let mine = |id: i64| -> Vec<Incident> { store.incidents(None, 50).unwrap().into_iter().filter(|i| i.source == format!("rule:{id}")).collect() };
    assert!(mine(health.id).is_empty(), "a healthy program is above the line");
    // The keeper's balance is 0.000000005 SOL in these transactions, far under 1 SOL.
    let low = mine(wallet.id);
    assert_eq!(low.len(), 1, "grouped into one incident");
    assert!(low[0].summary.contains("holds 0.0000 SOL") && low[0].summary.contains("below the 1 SOL"), "{}", low[0].summary);
    assert!(low[0].summary.contains(&keeper[..4]), "{}", low[0].summary);

    for sec in 360..480 {
        for i in 0..5 {
            n += 1;
            s.on_transaction(traffic(n, t0 + sec, i >= 3));
        }
        s.on_tick(t0 + sec + 1);
    }
    let dropped = mine(health.id);
    assert_eq!(dropped.len(), 1, "fires once when the score drops");
    assert!(dropped[0].summary.starts_with("Health score ") && dropped[0].summary.contains("below 70"), "{}", dropped[0].summary);
    assert!(dropped[0].summary.contains("Weakest check"), "{}", dropped[0].summary);

    // The score is sampled about once a minute and kept per hour, for summaries.
    let hours = store.rollups_between(PROGRAM, 0, i64::MAX).unwrap();
    let samples: u32 = hours.iter().map(|(_, r)| r.health_n).sum();
    assert!(samples >= 5, "{samples} samples over seven minutes");
    let lowest = hours.iter().filter_map(|(_, r)| r.health_min).min().unwrap();
    assert!(lowest < 70, "the dip is in the history: {lowest}");
    let _ = std::fs::remove_file(&dir);
}
