//! A dead feed must not be reported as a program outage. Contrast with a program that
//! really goes quiet while the chain keeps moving, which must be.

use chrono::{TimeZone, Utc};
use sentinel::engine::Sentinel;
use sentinel::model::*;
use sentinel::source::VortexSource;
use sentinel::store::Store;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
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
async fn a_dead_feed_pauses_detectors_instead_of_blaming_the_program() {
    let (s, store, tip) = sentinel("dead");
    let mut n = 0;
    warm_up(&s, &tip, &mut n);
    assert!(kinds(&store).is_empty(), "steady traffic raises nothing");

    // The feed dies: no transactions and the chain tip stops moving, for five minutes.
    for sec in T0 + 600..T0 + 900 {
        s.on_tick(sec + 1);
    }
    assert!(s.stream_health().stalled, "a frozen tip is reported as a stalled feed");
    assert!(!kinds(&store).contains(&IncidentKind::ActivityDrop), "the program didn't stop, the feed did");

    // It comes back. Traffic resumes at the old rate and nothing should look like a spike or a drop.
    for sec in T0 + 900..T0 + 1_100 {
        for _ in 0..5 {
            n += 1;
            s.on_transaction(tx(n, sec, true));
        }
        tip.store(2_000 + (sec - T0 - 900) as u64 * 2, Ordering::SeqCst);
        s.on_tick(sec + 1);
    }
    assert!(!s.stream_health().stalled);
    assert!(kinds(&store).is_empty(), "recovery is clean: {:?}", kinds(&store));
}

#[tokio::test]
async fn a_program_that_really_goes_quiet_is_still_reported() {
    let (s, store, tip) = sentinel("quiet");
    let mut n = 0;
    warm_up(&s, &tip, &mut n);

    // The chain keeps moving, but nothing touches the program for two minutes.
    for sec in T0 + 600..T0 + 720 {
        tip.store(1_000 + (sec - T0) as u64 * 2, Ordering::SeqCst);
        s.on_tick(sec + 1);
    }
    assert!(!s.stream_health().stalled, "the feed is fine");
    assert!(kinds(&store).contains(&IncidentKind::ActivityDrop), "the program did stop");
}
