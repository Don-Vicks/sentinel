//! Programs this one calls are tracked, watched for upgrades, and blamed when an upgrade precedes trouble.

use chrono::{TimeZone, Utc};
use sentinel::engine::Sentinel;
use sentinel::model::*;
use sentinel::posture::programdata_address;
use sentinel::source::VortexSource;
use std::sync::Mutex;
use sentinel::store::Store;
use std::sync::Arc;
use tokio::sync::broadcast;
use vortex::events::logs::Invocation;
use vortex::events::{AccountRef, Instruction, TxError, VortexTransaction};
use vortex::hub::HubStats;

const PROGRAM: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";
const LOADER: &str = "BPFLoaderUpgradeab1e11111111111111111111111";
const AUTHORITY: &str = "UpgradeAuthority1111111111111111111111111";
const NEW_AUTHORITY: &str = "NewAuthority11111111111111111111111111111";

const JUPITER: &str = "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4";

/// A source that remembers which accounts Sentinel asked it to stream.
struct Recorder {
    bus: broadcast::Sender<Arc<VortexTransaction>>,
    watched: Arc<Mutex<Vec<String>>>,
}

impl VortexSource for Recorder {
    fn subscribe(&self) -> broadcast::Receiver<Arc<VortexTransaction>> {
        self.bus.subscribe()
    }
    fn watch_programs(&self, programs: Vec<String>) {
        *self.watched.lock().unwrap() = programs;
    }
    fn health(&self) -> HubStats {
        HubStats { transactions: 0, last_slot: 0, last_transaction_at: None, started_at: Utc::now(), programs: vec![], subscribers: 0 }
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
        invocations: vec![
            Invocation {
                program_id: PROGRAM.into(),
                depth: 1,
                instruction: Some("Sell".into()),
                compute_consumed: Some(40_000),
                success: Some(ok),
                failure: (!ok).then(|| "custom program error: 0x1773".into()),
                ..Default::default()
            },
            // The program calls Jupiter, and the token program, which is not worth tracking.
            Invocation { program_id: JUPITER.into(), depth: 2, parent: Some(0), success: Some(ok), ..Default::default() },
            Invocation { program_id: "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA".into(), depth: 2, parent: Some(0), success: Some(true), ..Default::default() },
        ],
        logs: vec![],
        logs_truncated: false,
        token_balances: vec![],
        transfers: vec![],
        filters: vec![],
    })
}


/// An upgrade of `target`'s code by a key, not touching the monitored program at all.
fn upgrade_of(target: &str, signature: &str, second: i64) -> Arc<VortexTransaction> {
    use base64::Engine;
    let code = programdata_address(target).unwrap();
    let mut tx = (*traffic(0, second, true)).clone();
    tx.signature = signature.into();
    tx.invocations = vec![];
    tx.accounts = vec![account("Deployer1111111111111111111111111111111111", true), account(&code, false), account(target, false), account(AUTHORITY, true)];
    let accounts: Vec<String> = [code.as_str(), target, "Buffer11111111111111111111111111111111111", "Spill111111111111111111111111111111111111", "SysvarRent111111111111111111111111111111111", "SysvarC1ock11111111111111111111111111111111", AUTHORITY]
        .iter()
        .map(|a| a.to_string())
        .collect();
    tx.instructions = vec![vortex::events::Instruction {
        path: "0".into(),
        top_index: 0,
        inner_index: None,
        stack_height: 1,
        program_id: "BPFLoaderUpgradeab1e11111111111111111111111".into(),
        program_name: None,
        accounts,
        data: base64::engine::general_purpose::STANDARD.encode(3u32.to_le_bytes()),
        name: None,
        parsed: None,
    }];
    Arc::new(tx)
}

#[tokio::test]
async fn programs_this_one_calls_are_watched_and_blamed_after_an_upgrade() {
    let dir = std::env::temp_dir().join(format!("sentinel-deps-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    let store = Arc::new(Store::open(dir.to_str().unwrap()).unwrap());
    let (bus, _) = broadcast::channel(16);
    let watched = Arc::new(Mutex::new(Vec::new()));
    let s = Sentinel::new(store.clone(), Arc::new(Recorder { bus, watched: watched.clone() }), None, sentinel::pricing::PriceBook::new(), "http://ui".into()).unwrap();
    s.add_program(PROGRAM.into(), None).unwrap();
    let jupiter_code = programdata_address(JUPITER).unwrap();
    assert!(!watched.lock().unwrap().contains(&jupiter_code), "nothing is known about its dependencies yet");

    let t0 = 1_700_000_100i64; // a multiple of 30, so the dependency check runs
    assert_eq!(t0 % 30, 0);
    let mut n = 0;
    for sec in 0..300 {
        for _ in 0..5 {
            n += 1;
            s.on_transaction(traffic(n, t0 + sec, n % 50 != 0));
        }
        s.on_tick(t0 + sec + 1);
    }
    // It learned what the program calls, ignoring the token program, and now streams Jupiter's code account.
    let deps = s.dependencies(PROGRAM).unwrap();
    let list = deps["dependencies"].as_array().unwrap();
    assert_eq!(list.len(), 1, "{deps}");
    assert_eq!(list[0]["program_id"], JUPITER);
    assert_eq!(list[0]["calls"], 1500);
    assert_eq!(list[0]["watched_for_upgrades"], true);
    assert!(watched.lock().unwrap().contains(&jupiter_code), "the dependency's code account is streamed: {:?}", watched.lock().unwrap());

    // Jupiter is upgraded.
    s.on_transaction(upgrade_of(JUPITER, "jupiterUpgradeSig", t0 + 301));
    s.on_tick(t0 + 302);
    let change = store
        .incidents(Some(PROGRAM), 5)
        .unwrap()
        .into_iter()
        .find(|i| i.kind == IncidentKind::DependencyChange)
        .expect("dependency incident");
    assert_eq!(change.severity, Severity::Medium);
    assert_eq!(change.evidence["dependency"]["program_id"], JUPITER);
    assert!(change.title.contains("Pump.fun"), "{}", change.title);
    assert!(change.summary.contains("which Pump.fun calls"), "{}", change.summary);
    // It is not mistaken for traffic to the monitored program.
    assert_eq!(s.programs()[0].total_tx, 1500);

    // Trouble starts half a minute later and points at it.
    for sec in 302..332 {
        for i in 0..5 {
            n += 1;
            s.on_transaction(traffic(n, t0 + sec, i >= 3));
        }
        s.on_tick(t0 + sec + 1);
    }
    let spike = store.incidents(Some(PROGRAM), 20).unwrap().into_iter().find(|i| i.kind == IncidentKind::FailureSpike).expect("failure spike");
    let note = spike.evidence["deploy"]["note"].as_str().unwrap_or_default();
    assert!(note.contains("a program yours calls"), "{note}");
    assert_eq!(spike.evidence["deploy"]["incident_id"], change.id);
    let diagnosis = s.diagnose_incident(spike.id).unwrap().unwrap();
    assert!(diagnosis.likely_cause.unwrap().contains("upgrade"), "an upgrade that preceded it is blamed");

    // The upgrade of a program nobody calls is not our business.
    let before = store.incidents(None, 100).unwrap().len();
    s.on_transaction(upgrade_of("11111111111111111111111111111112", "unrelatedUpgrade", t0 + 400));
    assert_eq!(store.incidents(None, 100).unwrap().len(), before);
    let _ = std::fs::remove_file(&dir);
}
