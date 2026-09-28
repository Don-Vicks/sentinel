//! Sentinel's interpretation layer on real mainnet Pump.fun transactions.

use sentinel::analyze::{fingerprint, summarize};
use sentinel::pricing::PriceBook;
use sentinel::trace::{self, OwnerCache};
use serde_json::Value;
use std::collections::HashMap;
use vortex::events::VortexTransaction;
use vortex::geyser::decode::decode_transaction;
use vortex::geyser::rpc_frame::frame_from_rpc_json;

const PUMP: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";

fn load(name: &str) -> VortexTransaction {
    let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let f: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let resp = &f["response"];
    let frame = frame_from_rpc_json(f["signature"].as_str().unwrap(), resp["slot"].as_u64().unwrap(), resp).unwrap();
    decode_transaction(frame, vec![]).unwrap()
}

fn labels() -> HashMap<String, String> {
    HashMap::from([(PUMP.to_string(), "Pump.fun".to_string())])
}

#[tokio::test]
async fn failed_trade_fingerprint_and_narrative() {
    let tx = load("pump_failed");
    let fp = fingerprint(&tx).unwrap();
    assert_eq!(fp.program_id, PUMP);
    assert_eq!(fp.instruction.as_deref(), Some("Buy"));
    assert_eq!(fp.error, "TooMuchSolRequired");
    assert_eq!(fp.code, Some(6002));

    let s = summarize(&tx, PUMP, &PriceBook::new(), None);
    assert!(!s.success);
    assert!(s.instructions.contains(&"Buy".to_string()));

    let t = trace::build(&tx, &labels(), None, &OwnerCache::default(), &PriceBook::new(), None).await;
    let last = t.narrative.last().unwrap();
    assert!(last.contains("Pump.fun::Buy") && last.contains("TooMuchSolRequired"), "{last}");
    assert!(t.call_tree.iter().any(|c| c.program_id == PUMP && c.success == Some(false)));
}

#[tokio::test]
async fn successful_trade_value_flow() {
    let tx = load("pump_ok");
    assert!(fingerprint(&tx).is_none());
    let prices = PriceBook::new();
    prices.insert(sentinel::pricing::WSOL, 200.0, 0.0);
    let t = trace::build(&tx, &labels(), None, &OwnerCache::default(), &prices, None).await;
    let sol_flow = t.flows.iter().find(|f| f.symbol == "SOL").unwrap();
    assert!((sol_flow.usd.unwrap() - sol_flow.amount * 200.0).abs() < 1e-9);
    assert!(t.narrative.iter().any(|l| l.contains("SOL ($")), "{:?}", t.narrative);
    assert!(!t.reverted);
    assert!(t.flows.iter().any(|f| f.symbol == "SOL"));
    assert!(t.flows.iter().any(|f| f.symbol != "SOL"));
    // Every flow endpoint is a labelled party.
    for f in &t.flows {
        assert!(t.parties.iter().any(|p| p.address == f.from), "unlabelled {}", f.from);
        assert!(t.parties.iter().any(|p| p.address == f.to), "unlabelled {}", f.to);
    }
    assert!(!t.balance_changes.is_empty());
    assert!(t.narrative[0].contains("called"));
    println!("{:#?}", t.narrative);
}
