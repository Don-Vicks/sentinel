//! Alerts on Squads actions: a proposal approved, an execution on a given vault. v4 and v5.

use base64::Engine;
use chrono::{TimeZone, Utc};
use sentinel::engine::Sentinel;
use sentinel::model::*;
use sentinel::source::VortexSource;
use sentinel::squads::{vault_address, V4, V5};
use sentinel::store::Store;
use solana_sdk::pubkey::Pubkey;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use vortex::events::{AccountRef, Instruction, VortexTransaction};
use vortex::hub::HubStats;

const PROGRAM: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";
const MEMBER: &str = "Member11111111111111111111111111111111111";

struct FakeSource(broadcast::Sender<Arc<VortexTransaction>>, Arc<Mutex<Vec<String>>>);
impl VortexSource for FakeSource {
    fn subscribe(&self) -> broadcast::Receiver<Arc<VortexTransaction>> {
        self.0.subscribe()
    }
    fn watch_programs(&self, ids: Vec<String>) {
        *self.1.lock().unwrap() = ids;
    }
    fn health(&self) -> HubStats {
        HubStats { transactions: 0, last_slot: 0, last_transaction_at: None, started_at: Utc::now(), programs: vec![], subscribers: 0 }
    }
}

fn multisig() -> String {
    Pubkey::new_from_array([7; 32]).to_string()
}

fn account(pubkey: &str, signer: bool) -> AccountRef {
    AccountRef { pubkey: pubkey.into(), signer, writable: true, from_lookup_table: false, pre_lamports: 10, post_lamports: 5 }
}

/// A Squads call on the multisig that doesn't touch the watched program at all.
fn squads_call(signature: &str, program: &str, name: &str, extra: &[String], ok: bool) -> Arc<VortexTransaction> {
    let ms = multisig();
    let mut accounts = vec![ms.clone(), "Proposal111111111111111111111111111111111".to_string(), MEMBER.to_string()];
    accounts.extend(extra.iter().cloned());
    let disc = solana_sdk::hash::hashv(&[b"global:", name.as_bytes()]).to_bytes()[..8].to_vec();
    Arc::new(VortexTransaction {
        signature: signature.into(),
        slot: 1,
        index: 0,
        received_at: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
        success: ok,
        error: None,
        fee: 5000,
        compute_units: Some(1),
        compute_unit_limit: None,
        compute_unit_price: None,
        accounts: accounts.iter().map(|a| account(a, a == MEMBER)).collect(),
        instructions: vec![Instruction {
            path: "0".into(),
            top_index: 0,
            inner_index: None,
            stack_height: 1,
            program_id: program.into(),
            program_name: None,
            accounts,
            data: base64::engine::general_purpose::STANDARD.encode(disc),
            name: None,
            parsed: None,
        }],
        invocations: vec![],
        logs: vec![],
        logs_truncated: false,
        token_balances: vec![],
        transfers: vec![],
        filters: vec![],
    })
}

fn squads_rule(store: &Store, name: &str, actions: &str, vault_index: Option<u8>) -> AlertRule {
    store
        .create_rule(AlertRule {
            id: 0,
            owner: None,
            name: name.into(),
            program_id: Some(PROGRAM.into()),
            condition: Condition::Squads { multisig: multisig(), actions: actions.into(), vault_index, success_only: true },
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

fn hits(store: &Store, rule_id: i64) -> Vec<Incident> {
    store.incidents(None, 50).unwrap().into_iter().filter(|i| i.source == format!("rule:{rule_id}")).collect()
}

#[tokio::test]
async fn squads_rules_fire_on_actions_and_on_a_vault_in_v4_and_v5() {
    let dir = std::env::temp_dir().join(format!("sentinel-squads-rules-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    let store = Arc::new(Store::open(dir.to_str().unwrap()).unwrap());
    let (bus, _) = broadcast::channel(16);
    let watched = Arc::new(Mutex::new(Vec::new()));
    let s = Sentinel::new(store.clone(), Arc::new(FakeSource(bus, watched.clone())), None, sentinel::pricing::PriceBook::new(), "http://ui".into()).unwrap();
    s.add_program(PROGRAM.into(), None).unwrap();

    let approvals = squads_rule(&store, "Approvals", "proposal_approve|approve_proposal", None);
    let executions = squads_rule(&store, "Executions", "*execute*", None);
    let vault0 = squads_rule(&store, "Vault 0 executions", "*execute*", Some(0));
    let vault1 = squads_rule(&store, "Vault 1 executions", "*execute*", Some(1));
    let anything = squads_rule(&store, "Anything", "", None);
    s.reload_rules().unwrap();
    assert!(watched.lock().unwrap().contains(&multisig()), "the multisig joins the stream: {:?}", watched.lock().unwrap());

    // v4: a member approves a proposal. It touches the multisig, not the program.
    s.on_transaction(squads_call("approve4", V4, "proposal_approve", &[], true));
    // v5 names it differently.
    s.on_transaction(squads_call("approve5", V5, "approve_proposal", &[], true));
    s.flush();
    assert_eq!(hits(&store, approvals.id).len(), 1, "both versions' approvals share one open incident for the rule");
    assert!(hits(&store, executions.id).is_empty(), "an approval is not an execution");
    assert!(hits(&store, vault0.id).is_empty(), "a vote names no vault");
    assert_eq!(hits(&store, anything.id).len(), 1);
    let linked = store.incident_transactions(hits(&store, approvals.id)[0].id, 10).unwrap();
    assert_eq!(linked.len(), 2, "both approvals are evidence");

    // v4 execution that involves vault 0.
    let v0 = vault_address(V4, &multisig(), 0).unwrap();
    s.on_transaction(squads_call("exec4", V4, "vault_transaction_execute", &[v0], true));
    s.flush();
    assert_eq!(hits(&store, executions.id).len(), 1);
    assert_eq!(hits(&store, vault0.id).len(), 1, "the vault is in the transaction");
    assert!(hits(&store, vault1.id).is_empty(), "another vault's rule stays quiet");
    let incident = &hits(&store, vault0.id)[0];
    assert!(incident.summary.contains("Squads v4 vault_transaction_execute"), "{}", incident.summary);
    assert!(incident.summary.contains("vault 0"), "{}", incident.summary);
    assert!(incident.summary.contains("by Memb"), "names the member: {}", incident.summary);

    // v5 synchronous execution on vault 1.
    let v1 = vault_address(V5, &multisig(), 1).unwrap();
    s.on_transaction(squads_call("sync5", V5, "execute_transaction_sync", &[v1], true));
    s.flush();
    assert_eq!(hits(&store, vault1.id).len(), 1);
    assert!(hits(&store, vault1.id)[0].summary.contains("Squads v5 execute_transaction_sync"));

    // A failed action changed nothing.
    let before = store.incidents(None, 100).unwrap().len();
    s.on_transaction(squads_call("failed", V4, "vault_transaction_execute", &[], false));
    assert_eq!(store.incidents(None, 100).unwrap().len(), before);
    let _ = std::fs::remove_file(&dir);
}
