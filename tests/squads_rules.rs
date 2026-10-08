//! Alerts on Squads actions: a proposal approved, an execution on a given vault. v4 and v5.

use base64::Engine;
use chrono::{TimeZone, Utc};
use sentinel::engine::Sentinel;
use sentinel::model::*;
use sentinel::pricing::PriceBook;
use sentinel::source::VortexSource;
use sentinel::squads::{vault_address, V4, V5};
use sentinel::store::Store;
use solana_sdk::pubkey::Pubkey;
use axum::{routing::post, Json, Router};
use serde_json::{json, Value};
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

fn proposal_addr() -> String {
    Pubkey::new_from_array([8; 32]).to_string()
}

fn account(pubkey: &str, signer: bool) -> AccountRef {
    AccountRef { pubkey: pubkey.into(), signer, writable: true, from_lookup_table: false, pre_lamports: 10, post_lamports: 5 }
}

/// A Squads call on the multisig that doesn't touch the watched program at all.
fn squads_call(signature: &str, program: &str, name: &str, extra: &[String], ok: bool) -> Arc<VortexTransaction> {
    let ms = multisig();
    let mut accounts = vec![ms.clone(), proposal_addr(), MEMBER.to_string()];
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

/// A v4 `Multisig` with a threshold of 3 of 5.
fn multisig_account() -> Vec<u8> {
    let mut d = vec![0u8; 8 + 64];
    d.extend(3u16.to_le_bytes());
    d.extend(0u32.to_le_bytes());
    d.extend(9u64.to_le_bytes());
    d.extend(4u64.to_le_bytes());
    d.push(0);
    d.push(253);
    d.extend(5u32.to_le_bytes());
    for i in 0..5u8 {
        d.extend([i + 1; 32]);
        d.push(7);
    }
    d
}

/// A v4 `Proposal` of this multisig, active, with `approved` approvals.
fn proposal_account(approved: u8) -> Vec<u8> {
    let mut d = vec![0u8; 8];
    d.extend([7u8; 32]); // the multisig
    d.extend(12u64.to_le_bytes());
    // The program marks it approved as soon as the threshold is reached.
    d.push(if approved >= 3 { 3 } else { 1 });
    d.extend(1_700_000_000i64.to_le_bytes());
    d.push(254);
    d.extend((approved as u32).to_le_bytes());
    for i in 0..approved {
        d.extend([i + 1; 32]);
    }
    d.extend(0u32.to_le_bytes());
    d.extend(0u32.to_le_bytes());
    d.extend([0u8; 64]);
    d
}

/// An RPC that serves the multisig and, for the proposal address, whatever `approved` says now.
async fn mock_rpc(approved: Arc<Mutex<u8>>) -> String {
    let app = Router::new().route(
        "/",
        post(move |Json(req): Json<Value>| {
            let approved = approved.clone();
            async move {
                let enc = |d: Vec<u8>| base64::engine::general_purpose::STANDARD.encode(d);
                let result = match req["method"].as_str().unwrap_or_default() {
                    "getVersion" => json!({ "solana-core": "1.18.26", "feature-set": 1 }),
                    "getAccountInfo" => {
                        let address = req["params"][0].as_str().unwrap_or_default();
                        let data = if address == multisig() { Some(multisig_account()) } else if address == proposal_addr() { Some(proposal_account(*approved.lock().unwrap())) } else { None };
                        match data {
                            Some(d) => json!({ "context": { "slot": 1 }, "value": { "data": [enc(d), "base64"], "executable": false, "lamports": 1, "owner": V4, "rentEpoch": 0, "space": 400 } }),
                            None => json!({ "context": { "slot": 1 }, "value": null }),
                        }
                    }
                    _ => Value::Null,
                };
                Json(json!({ "jsonrpc": "2.0", "id": req["id"].clone(), "result": result }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

#[tokio::test]
async fn a_vote_says_how_many_have_approved_and_the_next_vote_replaces_it() {
    let approved = Arc::new(Mutex::new(2u8));
    let url = mock_rpc(approved.clone()).await;
    let dir = std::env::temp_dir().join(format!("sentinel-squads-proposal-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    let store = Arc::new(Store::open(dir.to_str().unwrap()).unwrap());
    let (bus, _) = broadcast::channel(16);
    let rpc = Arc::new(solana_client::nonblocking::rpc_client::RpcClient::new(url));
    let s = Sentinel::new(store.clone(), Arc::new(FakeSource(bus, Arc::new(Mutex::new(Vec::new())))), Some(rpc), PriceBook::new(), "http://ui".into()).unwrap();
    s.add_program(PROGRAM.into(), None).unwrap();
    let votes = squads_rule(&store, "Votes", "proposal_approve", None);
    s.reload_rules().unwrap();

    s.on_transaction(squads_call("vote1", V4, "proposal_approve", &[], true));
    s.flush();
    let incident = hits(&store, votes.id).remove(0);
    s.enrich_proposal(incident.id, &multisig(), &[proposal_addr()]).await.unwrap();
    s.on_tick(1_700_000_001);
    let got = store.incident(incident.id).unwrap().unwrap();
    assert!(got.summary.contains("proposal 12: 2 of 3 approved"), "{}", got.summary);
    assert_eq!(got.evidence["proposal"]["state"]["status"], "active");
    assert_eq!(got.evidence["proposal"]["state"]["approved"].as_array().unwrap().len(), 2);
    assert_eq!(got.evidence["proposal"]["requires"]["threshold"], 3);

    // The third approval: the same incident, now saying it is ready, not both.
    *approved.lock().unwrap() = 3;
    s.enrich_proposal(incident.id, &multisig(), &[proposal_addr()]).await.unwrap();
    s.on_tick(1_700_000_002);
    let got = store.incident(incident.id).unwrap().unwrap();
    assert!(got.summary.contains("3 of 3 approved, ready to execute"), "{}", got.summary);
    assert_eq!(got.summary.matches("proposal 12").count(), 1, "{}", got.summary);
    assert!(!got.summary.contains("2 of 3"), "{}", got.summary);

    // An account that isn't a proposal of this multisig is ignored, not misread.
    s.enrich_proposal(incident.id, &multisig(), &[multisig()]).await.unwrap();
    s.on_tick(1_700_000_003);
    assert!(store.incident(incident.id).unwrap().unwrap().summary.contains("3 of 3 approved"));
    let _ = std::fs::remove_file(&dir);
}
