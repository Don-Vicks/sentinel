//! A watched vault that loses a large share of its balance becomes an incident.

use chrono::{TimeZone, Utc};
use sentinel::engine::Sentinel;
use sentinel::model::*;
use sentinel::source::VortexSource;
use sentinel::store::Store;
use std::sync::Arc;
use tokio::sync::broadcast;
use vortex::events::logs::Invocation;
use vortex::events::{AccountRef, TokenBalanceChange, TxError, VortexTransaction};
use vortex::hub::HubStats;

const PROGRAM: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";
const VAULT: &str = "Vau1t11111111111111111111111111111111111111";
const USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

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


/// A successful transaction that moves `vault` from `pre` to `post` USDC.
fn vault_move(n: u64, second: i64, pre: f64, post: f64) -> Arc<VortexTransaction> {
    let mut tx = (*traffic(n, second, true)).clone();
    tx.token_balances = vec![TokenBalanceChange {
        account: VAULT.into(),
        owner: Some("VaultAuthorityPda1111111111111111111111111".into()),
        mint: USDC.into(),
        decimals: 6,
        pre,
        post,
        delta: post - pre,
    }];
    Arc::new(tx)
}

#[tokio::test]
async fn a_vault_losing_a_large_share_of_its_balance_opens_an_incident() {
    let dir = std::env::temp_dir().join(format!("sentinel-vaults-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    let store = Arc::new(Store::open(dir.to_str().unwrap()).unwrap());
    let (bus, _) = broadcast::channel(16);
    let s = Sentinel::new(store.clone(), Arc::new(FakeSource(bus)), None, sentinel::pricing::PriceBook::new(), "http://ui".into()).unwrap();
    s.add_program(PROGRAM.into(), None).unwrap();
    s.set_vaults(PROGRAM, vec![VAULT.into()]).unwrap();
    let t0 = 1_700_000_000i64;

    // Everyday churn: 10% out, then most of it back in. Net flow never reaches the threshold.
    s.on_transaction(vault_move(1, t0, 1_000_000.0, 900_000.0));
    s.on_transaction(vault_move(2, t0 + 5, 900_000.0, 1_050_000.0));
    s.on_tick(t0 + 6);
    assert!(store.incidents(None, 10).unwrap().is_empty(), "net inflow is not a drain");

    // Then it drains. After 150K more leaves, the window's net is 100K of 1M (10%): under the line.
    s.on_transaction(vault_move(3, t0 + 20, 1_050_000.0, 900_000.0));
    assert!(store.incidents(None, 10).unwrap().is_empty(), "10% net outflow is under the 20% threshold");
    s.on_transaction(vault_move(4, t0 + 40, 900_000.0, 400_000.0));
    s.on_tick(t0 + 41);
    s.flush();
    let drain = store
        .incidents(Some(PROGRAM), 10)
        .unwrap()
        .into_iter()
        .find(|i| i.kind == IncidentKind::VaultDrain)
        .expect("vault drain incident");
    // Net over the window: 1M -> 400K, i.e. 600K or 60% of what it held at the start.
    assert_eq!(drain.severity, Severity::Critical);
    assert!((drain.observed.unwrap() - 60.0).abs() < 0.5, "{:?}", drain.observed);
    assert_eq!(drain.evidence["vault"]["account"], VAULT);
    assert_eq!(drain.evidence["vault"]["symbol"], "USDC");
    assert!(drain.summary.contains("USDC") && drain.summary.contains("60%"), "{}", drain.summary);
    assert!(drain.explanation.contains("threshold"), "{}", drain.explanation);
    assert!(!store.incident_transactions(drain.id, 10).unwrap().is_empty());

    // The dashboard sees the vault, its balance and the net flow.
    let status = s.vault_status(PROGRAM).unwrap();
    assert_eq!(status["vaults"][0]["account"], VAULT);
    assert_eq!(status["vaults"][0]["balance"], 400_000.0);

    // It can be switched off.
    let mut cfg = DetectionConfig::default();
    cfg.drain_enabled = false;
    cfg.vaults = vec![VAULT.into()];
    s.update_program(PROGRAM, None, Some(cfg)).unwrap();
    let before = store.incidents(None, 100).unwrap().len();
    s.on_transaction(vault_move(5, t0 + 60, 400_000.0, 10_000.0));
    assert_eq!(store.incidents(None, 100).unwrap().len(), before);
    let _ = std::fs::remove_file(&dir);
}
