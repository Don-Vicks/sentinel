//! Throughput of the two hot paths, on real mainnet Pump.fun transactions:
//! the Vortex decoder (Yellowstone frame -> VortexTransaction) and the
//! Sentinel engine (metrics, detectors, incident linking into SQLite).
//!
//!     cargo run --release --example bench

use chrono::{TimeZone, Utc};
use sentinel::engine::Sentinel;
use sentinel::pricing::PriceBook;
use sentinel::source::VortexSource;
use sentinel::store::Store;
use serde_json::Value;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::broadcast;
use vortex::events::VortexTransaction;
use vortex::geyser::decode::decode_transaction;
use vortex::geyser::rpc_frame::frame_from_rpc_json;
use vortex::hub::HubStats;

const PUMP: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";

struct NullSource(broadcast::Sender<Arc<VortexTransaction>>);
impl VortexSource for NullSource {
    fn subscribe(&self) -> broadcast::Receiver<Arc<VortexTransaction>> {
        self.0.subscribe()
    }
    fn watch_programs(&self, _: Vec<String>) {}
    fn health(&self) -> HubStats {
        HubStats { transactions: 0, last_slot: 0, last_transaction_at: None, started_at: Utc::now(), programs: vec![], subscribers: 0 }
    }
}

fn frames() -> Vec<yellowstone_grpc_proto::prelude::SubscribeUpdateTransaction> {
    ["pump_ok", "pump_failed", "pump_v1"]
        .iter()
        .map(|name| {
            let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
            let f: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            let resp = &f["response"];
            frame_from_rpc_json(f["signature"].as_str().unwrap(), resp["slot"].as_u64().unwrap(), resp).unwrap()
        })
        .collect()
}

#[tokio::main]
async fn main() {
    let frames = frames();

    // --- decoder ---
    let n = 60_000;
    let started = Instant::now();
    let mut decoded = Vec::with_capacity(frames.len());
    for i in 0..n {
        let tx = decode_transaction(frames[i % frames.len()].clone(), vec![]).unwrap();
        if i < frames.len() {
            decoded.push(tx);
        }
    }
    let secs = started.elapsed().as_secs_f64();
    println!("decoder: {n} tx in {secs:.2}s = {:.0} tx/s ({:.1} µs/tx, incl. frame clone)", n as f64 / secs, secs * 1e6 / n as f64);

    // --- engine ---
    let db = std::env::temp_dir().join(format!("sentinel-bench-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let store = Arc::new(Store::open(db.to_str().unwrap()).unwrap());
    let (bus, _) = broadcast::channel(4);
    let s = Sentinel::new(store.clone(), Arc::new(NullSource(bus)), None, PriceBook::new(), "http://x".into()).unwrap();
    s.add_program(PUMP.into(), None).unwrap();

    // 20 minutes of simulated time at 500 tx/s with a 2-minute failure storm.
    let tps = 500u64;
    let t0 = 1_700_000_000i64;
    let seconds = 1_200i64;
    let (ok, failed) = (&decoded[0], &decoded[1]);
    let started = Instant::now();
    let mut count = 0u64;
    for sec in 0..seconds {
        let storm = (600..720).contains(&sec);
        for i in 0..tps {
            let base = if (storm && i % 2 == 0) || i % 25 == 0 { failed } else { ok };
            let mut tx = base.clone();
            tx.signature = format!("bench{count}");
            tx.received_at = Utc.timestamp_opt(t0 + sec, 0).unwrap();
            s.on_transaction(Arc::new(tx));
            count += 1;
        }
        s.on_tick(t0 + sec + 1);
    }
    let secs = started.elapsed().as_secs_f64();
    let incidents = store.incidents(None, 50).unwrap();
    println!(
        "engine:  {count} tx ({} simulated minutes at {tps} tx/s) in {secs:.2}s = {:.0} tx/s, {} incident(s): {:?}",
        seconds / 60,
        count as f64 / secs,
        incidents.len(),
        incidents.iter().map(|i| i.kind.as_str()).collect::<Vec<_>>()
    );
    let _ = std::fs::remove_file(&db);
}
