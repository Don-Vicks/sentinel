//! Anchor IDL decoding against Pump.fun's real on-chain IDL account and a
//! real mainnet Buy.

use base64::Engine;
use sentinel::idl::{decode_idl_account, idl_address, Idl};
use serde_json::Value;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use vortex::geyser::decode::decode_transaction;
use vortex::geyser::rpc_frame::frame_from_rpc_json;

const PUMP: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";

fn pump_idl() -> Idl {
    let data = std::fs::read(format!("{}/tests/fixtures/pump_idl_account.bin", env!("CARGO_MANIFEST_DIR"))).unwrap();
    Idl::parse(PUMP, &decode_idl_account(&data).unwrap()).unwrap()
}

#[test]
fn derives_the_anchor_idl_address() {
    let addr = idl_address(&Pubkey::from_str(PUMP).unwrap());
    assert_eq!(addr.to_string(), "AYgC53tU5BbP2NAnv5nConJxAdpQZctvmZK88pu69xRs");
}

#[test]
fn decodes_real_buy_and_errors() {
    let idl = pump_idl();
    assert_eq!(idl.error(6002).unwrap().name, "TooMuchSolRequired");
    assert!(idl.error(6002).unwrap().message.as_deref().unwrap().contains("slippage"));

    let path = format!("{}/tests/fixtures/pump_ok.json", env!("CARGO_MANIFEST_DIR"));
    let f: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let resp = &f["response"];
    let frame = frame_from_rpc_json(f["signature"].as_str().unwrap(), resp["slot"].as_u64().unwrap(), resp).unwrap();
    let tx = decode_transaction(frame, vec![]).unwrap();

    let ix = tx.instructions.iter().find(|i| i.program_id == PUMP && i.name.as_deref() == Some("Buy")).unwrap();
    let data = base64::engine::general_purpose::STANDARD.decode(&ix.data).unwrap();
    let decoded = idl.decode_instruction(&data, &ix.accounts).expect("matches an IDL instruction");
    assert_eq!(decoded.name, "buy");
    assert!(decoded.args["amount"].as_u64().unwrap() > 0);
    assert!(decoded.args["max_sol_cost"].as_u64().unwrap() > 0);
    assert_eq!(decoded.accounts[3].name, "bonding_curve");
    assert_eq!(decoded.accounts[6].name, "user");
    println!("{}", serde_json::to_string_pretty(&decoded).unwrap());
}

fn load_tx(name: &str) -> vortex::events::VortexTransaction {
    let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let f: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let resp = &f["response"];
    let frame = frame_from_rpc_json(f["signature"].as_str().unwrap(), resp["slot"].as_u64().unwrap(), resp).unwrap();
    decode_transaction(frame, vec![]).unwrap()
}

#[tokio::test]
async fn trace_uses_idl_for_args_accounts_and_errors() {
    use sentinel::idl::IdlRegistry;
    use sentinel::pricing::PriceBook;
    use sentinel::trace::{self, OwnerCache};

    let registry = IdlRegistry::new(None);
    registry.insert(pump_idl());
    let labels = std::collections::HashMap::from([(PUMP.to_string(), "Pump.fun".to_string())]);

    let tx = load_tx("pump_failed");
    let t = trace::build(&tx, &labels, None, &OwnerCache::default(), &PriceBook::new(), Some(&registry)).await;
    let detail = t.error_detail.expect("IDL error detail");
    assert_eq!((detail.code, detail.name.as_str()), (6002, "TooMuchSolRequired"));
    let buy = t.decoded.iter().find(|d| d.program_id == PUMP).expect("decoded pump ix");
    assert_eq!(buy.instruction.name, "buy");
    assert!(buy.instruction.accounts.iter().any(|a| a.name == "bonding_curve"));

    // Without an AnchorError log line, the code alone is named from the IDL.
    let mut bare = tx.clone();
    bare.logs.retain(|l| !l.contains("AnchorError"));
    if let Some(e) = bare.error.as_mut() {
        e.name = Some("Custom(6002)".into());
    }
    let fp = sentinel::analyze::fingerprint_with(&bare, Some(&registry)).unwrap();
    assert_eq!(fp.error, "TooMuchSolRequired");
    assert_eq!(sentinel::analyze::fingerprint(&bare).unwrap().error, "Custom(6002)");
}
