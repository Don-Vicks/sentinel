//! Program upgrades and authority changes become incidents, and a failure
//! spike that starts right after an upgrade points at it.

use chrono::{TimeZone, Utc};
use sentinel::engine::Sentinel;
use sentinel::model::*;
use sentinel::posture::programdata_address;
use sentinel::source::VortexSource;
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

/// A call to the upgradeable loader. `variant` is the loader's instruction number.
fn loader_call(signature: &str, second: i64, ok: bool, variant: u32, accounts: Vec<String>) -> Arc<VortexTransaction> {
    use base64::Engine;
    let mut tx = (*traffic(0, second, true)).clone();
    tx.signature = signature.into();
    tx.success = ok;
    tx.accounts = accounts.iter().map(|a| account(a, a == AUTHORITY)).collect();
    tx.accounts.insert(0, account("Deployer1111111111111111111111111111111111", true));
    tx.invocations = vec![];
    tx.instructions = vec![Instruction {
        path: "0".into(),
        top_index: 0,
        inner_index: None,
        stack_height: 1,
        program_id: LOADER.into(),
        program_name: None,
        accounts,
        data: base64::engine::general_purpose::STANDARD.encode(variant.to_le_bytes()),
        name: None,
        parsed: None,
    }];
    Arc::new(tx)
}

fn upgrade(signature: &str, second: i64, ok: bool) -> Arc<VortexTransaction> {
    let pd = programdata_address(PROGRAM).unwrap();
    // Upgrade: [programdata, program, buffer, spill, rent, clock, authority]
    let accounts = [pd.as_str(), PROGRAM, "Buffer11111111111111111111111111111111111", "Spill111111111111111111111111111111111111", "SysvarRent111111111111111111111111111111111", "SysvarC1ock11111111111111111111111111111111", AUTHORITY];
    loader_call(signature, second, ok, 3, accounts.iter().map(|a| a.to_string()).collect())
}

#[tokio::test]
async fn upgrades_and_authority_changes_open_incidents_and_explain_what_follows() {
    let dir = std::env::temp_dir().join(format!("sentinel-upgrades-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    let store = Arc::new(Store::open(dir.to_str().unwrap()).unwrap());
    let (bus, _) = broadcast::channel(16);
    let s = Sentinel::new(store.clone(), Arc::new(FakeSource(bus)), None, sentinel::pricing::PriceBook::new(), "http://ui".into()).unwrap();
    s.add_program(PROGRAM.into(), None).unwrap();

    let t0 = 1_700_000_000i64;
    let mut n = 0;
    for sec in t0..t0 + 300 {
        for _ in 0..5 {
            n += 1;
            s.on_transaction(traffic(n, sec, n % 50 != 0));
        }
        s.on_tick(sec + 1);
    }
    assert!(store.incidents(None, 10).unwrap().is_empty(), "no incident on healthy traffic");

    // A failed upgrade changed nothing.
    s.on_transaction(upgrade("failedUpgrade", t0 + 300, false));
    assert!(store.incidents(None, 10).unwrap().is_empty());

    // The upgrade lands.
    s.on_transaction(upgrade("upgradeSig", t0 + 301, true));
    s.on_tick(t0 + 302);
    s.flush();
    let upgrade_incident = store
        .incidents(Some(PROGRAM), 10)
        .unwrap()
        .into_iter()
        .find(|i| i.kind == IncidentKind::AuthorityChange)
        .expect("upgrade incident");
    assert_eq!(upgrade_incident.severity, Severity::High);
    assert!(upgrade_incident.title.starts_with("Program upgraded"), "{}", upgrade_incident.title);
    assert_eq!(upgrade_incident.evidence["authority"]["action"], "upgrade");
    assert_eq!(upgrade_incident.evidence["authority"]["authority"], AUTHORITY);
    assert_eq!(upgrade_incident.evidence["authority"]["signature"], "upgradeSig");
    assert_eq!(store.incident_transactions(upgrade_incident.id, 10).unwrap().len(), 1);

    // Failures begin half a minute later.
    for sec in t0 + 302..t0 + 332 {
        for i in 0..5 {
            n += 1;
            s.on_transaction(traffic(n, sec, i >= 3));
        }
        s.on_tick(sec + 1);
    }
    let spike = store
        .incidents(Some(PROGRAM), 20)
        .unwrap()
        .into_iter()
        .find(|i| i.kind == IncidentKind::FailureSpike)
        .expect("failure spike");
    let deploy = &spike.evidence["deploy"];
    assert_eq!(deploy["signature"], "upgradeSig", "{}", spike.evidence);
    assert_eq!(deploy["incident_id"], upgrade_incident.id);
    assert!(spike.explanation.contains("after the program was upgraded"), "{}", spike.explanation);
    // The correlation survives the evidence refreshes that follow.
    assert!(spike.evidence["fingerprints"].as_array().is_some_and(|f| !f.is_empty()));

    // An authority change touches only the ProgramData account, not the program.
    let pd = programdata_address(PROGRAM).unwrap();
    let set_authority = loader_call("setAuthoritySig", t0 + 340, true, 4, vec![pd, AUTHORITY.into(), NEW_AUTHORITY.into()]);
    // (Strip the program account so only the code account is touched.)
    s.on_transaction(set_authority);
    s.on_tick(t0 + 341);
    let moved = store
        .incidents(Some(PROGRAM), 20)
        .unwrap()
        .into_iter()
        .find(|i| i.kind == IncidentKind::AuthorityChange && i.id != upgrade_incident.id)
        .expect("authority change incident");
    assert_eq!(moved.severity, Severity::Critical);
    assert_eq!(moved.title, "Upgrade authority changed · Pump.fun");
    assert_eq!(moved.evidence["authority"]["new_authority"], NEW_AUTHORITY);
    assert!(moved.summary.contains("NewA"), "{}", moved.summary);

    // Settings can turn the detector off per program.
    let mut cfg = DetectionConfig::default();
    cfg.authority_enabled = false;
    let before = store.incidents(None, 100).unwrap().len();
    s.update_program(PROGRAM, None, Some(cfg)).unwrap();
    s.on_transaction(upgrade("upgradeIgnored", t0 + 400, true));
    assert_eq!(store.incidents(None, 100).unwrap().len(), before);
    let _ = std::fs::remove_file(&dir);
}
