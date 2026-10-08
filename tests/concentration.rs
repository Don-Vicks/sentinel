//! One wallet suddenly sending most of a program's traffic.

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


/// A successful transaction paid for by `payer`.
fn from(payer: &str, n: u64, second: i64) -> Arc<VortexTransaction> {
    let mut tx = (*traffic(n, second, true)).clone();
    tx.accounts[0].pubkey = payer.to_string();
    Arc::new(tx)
}

fn run(share_before: u64) -> Vec<Incident> {
    let dir = std::env::temp_dir().join(format!("sentinel-conc-{share_before}-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    let store = Arc::new(Store::open(dir.to_str().unwrap()).unwrap());
    let (bus, _) = broadcast::channel(16);
    let s = Sentinel::new(store.clone(), Arc::new(FakeSource(bus)), None, sentinel::pricing::PriceBook::new(), "http://ui".into()).unwrap();
    s.add_program(PROGRAM.into(), None).unwrap();
    let t0 = 1_700_000_000i64;
    let bot = "Bot1111111111111111111111111111111111111111";
    let mut n = 0u64;
    // Ten minutes of ordinary traffic, in which `share_before` of every 10 transactions come from the bot.
    for sec in 0..600 {
        for i in 0..5u64 {
            n += 1;
            let payer = if (n % 10) < share_before { bot.to_string() } else { format!("Payer{:0>39}", (n * 7 + i) % 80) };
            s.on_transaction(from(&payer, n, t0 + sec));
        }
        s.on_tick(t0 + sec + 1);
    }
    // Then the bot sends almost everything for a minute.
    for sec in 600..670 {
        for i in 0..5u64 {
            n += 1;
            let payer = if i < 4 { bot.to_string() } else { format!("Payer{:0>39}", n % 80) };
            s.on_transaction(from(&payer, n, t0 + sec));
        }
        s.on_tick(t0 + sec + 1);
    }
    s.flush();
    let found = store.incidents(Some(PROGRAM), 20).unwrap().into_iter().filter(|i| i.kind == IncidentKind::BotActivity).collect();
    let _ = std::fs::remove_file(&dir);
    found
}

#[test]
fn a_wallet_that_suddenly_sends_most_of_the_traffic_is_flagged() {
    let found = run(0);
    assert_eq!(found.len(), 1, "{found:?}");
    let inc = &found[0];
    assert!(inc.summary.contains("One wallet sent 8"), "80% of the traffic: {}", inc.summary);
    assert!(inc.explanation.contains("Bot1111"), "names the wallet: {}", inc.explanation);
    assert_eq!(inc.kind.title(), "Traffic concentrated in one wallet");
    assert!(inc.affected_count > 100, "the wallet's transactions are linked: {}", inc.affected_count);
    assert_eq!(inc.affected_wallets, 1);
}

#[test]
fn a_program_that_is_always_mostly_one_wallet_is_left_alone() {
    // The same bot was 70% of the traffic all along, so 80% is not a change worth raising.
    assert!(run(7).is_empty());
}
