//! Squads parsers against real mainnet accounts and transactions, fetched with `getAccountInfo`
//! and `getTransaction` and kept in tests/fixtures. Nothing here is synthetic.

use sentinel::squads::{self, V4, V5};
use serde_json::Value;
use vortex::events::VortexTransaction;
use vortex::geyser::decode::decode_transaction;
use vortex::geyser::rpc_frame::frame_from_rpc_json;

fn bytes(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn tx(name: &str) -> VortexTransaction {
    let f: Value = serde_json::from_slice(&bytes(name)).unwrap();
    let resp = &f["response"];
    let frame = frame_from_rpc_json(f["signature"].as_str().unwrap(), resp["slot"].as_u64().unwrap(), resp).unwrap();
    decode_transaction(frame, vec![]).unwrap()
}

#[test]
fn real_squads_v4_accounts_parse() {
    let ms = squads::parse_multisig(&bytes("squads_v4_multisig.bin")).expect("a real v4 multisig");
    println!("{ms:?}");
    assert!(ms.threshold >= 1 && ms.members >= ms.threshold as usize);

    let p = squads::parse_proposal(V4, &bytes("squads_v4_proposal.bin")).expect("a real v4 proposal");
    println!("{p:?}");
    assert_eq!(p.multisig, "GKk5YnnK8fEzz3yYhitPSLbKcGKDfaKCvrVomtksB9AC", "the proposal belongs to the multisig it was fetched for");
    assert!(p.transaction_index > 0);
    println!("{}", p.describe(Some(ms.threshold)));
}

#[test]
fn real_squads_v5_settings_parse_and_a_policy_account_is_refused() {
    let s = squads::parse_settings(&bytes("squads_v5_settings.bin")).expect("a real v5 settings account");
    println!("{s:?}");
    assert!(s.threshold >= 1 && s.members >= s.threshold as usize);
    // The smart account program also has policy accounts, which are not multisig settings: never misread one.
    assert!(squads::parse_settings(&bytes("squads_v5_policy.bin")).is_none());
}

#[test]
fn real_squads_executions_are_recognised_and_name_their_vault() {
    // v4 vault_transaction_execute
    let t = tx("squads_v4_execute.json");
    let ms = "E55qV8ztG3hGJhX4rtnN32CdVUEh1ky37H8a2eWa6SWQ";
    let actions = squads::actions(&t, ms);
    println!("{actions:?}");
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].instruction, Some("vault_transaction_execute"));
    assert!(actions[0].member.is_some(), "the signing member");
    let vault = (0..8u8).find(|i| squads::vault_address(V4, ms, *i).is_some_and(|a| t.touches(&a)));
    println!("v4 vault index {vault:?}");
    assert!(vault.is_some(), "one of the multisig's first vaults is in the transaction");

    // v5 execute_transaction_sync on a settings account
    let t = tx("squads_v5_sync.json");
    let settings = "Aod4xZC3CDL5WoQTUtHmmzCATGhHU6TvVPAqx1qJtRjQ";
    let actions = squads::actions(&t, settings);
    println!("{actions:?}");
    assert!(actions.iter().any(|a| a.instruction == Some("execute_transaction_sync") && a.program_id == V5));
    let vault = (0..8u8).find(|i| squads::vault_address(V5, settings, *i).is_some_and(|a| t.touches(&a)));
    println!("v5 vault index {vault:?}");
    assert!(vault.is_some(), "one of the smart account's first vaults is in the transaction");

    // v5 execute_transaction on a policy account (recognised; policies have their own addresses)
    let t = tx("squads_v5_policy_execute.json");
    let actions = squads::actions(&t, "31ZP5Fhes1vQ49RwpVNGcyis4YJX8cNhbNyk5jNzThF2");
    assert!(actions.iter().any(|a| a.instruction == Some("execute_transaction")));
}

#[test]
fn a_real_v5_proposal_parses() {
    let p = squads::parse_proposal(V5, &bytes("squads_v5_proposal.bin")).expect("a real v5 proposal");
    println!("{p:?}");
    assert!(!p.multisig.is_empty() && p.transaction_index > 0);
    // Read as v4 it must not pass for a proposal: the rent collector shifts every field.
    let as_v4 = squads::parse_proposal(V4, &bytes("squads_v5_proposal.bin"));
    assert!(as_v4.is_none(), "read as v4: {as_v4:?}");
}
