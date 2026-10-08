//! Runs a program's recent real mainnet transactions through Sentinel and reports what it made of
//! them: instructions, decoded events, the rules its IDL suggests, incidents, health. Read-only
//! (public RPC, nothing is sent anywhere); a quick way to see that the decoding fits the real chain.
//!
//!     cargo run --release --example verify_mainnet -- [program] [--rpc URL] [--count 30]
//!
//! Defaults: Pump.fun, the public mainnet RPC (rate limited, so keep --count small), 30 transactions.

use anyhow::{bail, Result};
use chrono::Utc;
use sentinel::engine::Sentinel;
use sentinel::model::*;
use sentinel::source::VortexSource;
use sentinel::store::Store;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use vortex::events::VortexTransaction;
use vortex::geyser::decode::decode_transaction;
use vortex::geyser::rpc_frame::frame_from_rpc_json;
use vortex::hub::HubStats;

struct Quiet(broadcast::Sender<Arc<VortexTransaction>>);
impl VortexSource for Quiet {
    fn subscribe(&self) -> broadcast::Receiver<Arc<VortexTransaction>> {
        self.0.subscribe()
    }
    fn watch_programs(&self, _: Vec<String>) {}
    fn health(&self) -> HubStats {
        HubStats { transactions: 0, last_slot: 0, last_transaction_at: None, started_at: Utc::now(), programs: vec![], subscribers: 0 }
    }
}

async fn call(http: &reqwest::Client, url: &str, method: &str, params: Value) -> Result<Value> {
    for attempt in 0..6u64 {
        let res = http.post(url).json(&json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params })).send().await?;
        if res.status() == 429 {
            tokio::time::sleep(std::time::Duration::from_secs(2 * (attempt + 1))).await;
            continue;
        }
        let body: Value = res.json().await?;
        if let Some(e) = body.get("error") {
            bail!("{method}: {e}");
        }
        return Ok(body["result"].clone());
    }
    bail!("{method}: rate limited")
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut program = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P".to_string();
    let mut url = "https://api.mainnet-beta.solana.com".to_string();
    let mut count = 30usize;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--rpc" => url = args.next().expect("--rpc needs a URL"),
            "--count" => count = args.next().and_then(|c| c.parse().ok()).expect("--count needs a number"),
            p => program = p.to_string(),
        }
    }
    let http = reqwest::Client::new();
    println!("Fetching the last {count} transactions of {program} from {url} …");
    let sigs = call(&http, &url, "getSignaturesForAddress", json!([program, { "limit": count }])).await?;
    let mut txs: Vec<VortexTransaction> = Vec::new();
    let mut skipped = 0;
    for s in sigs.as_array().cloned().unwrap_or_default() {
        let sig = s["signature"].as_str().unwrap_or_default();
        // The newest transaction version the node knows; older ones are served as before.
        let res = match call(&http, &url, "getTransaction", json!([sig, { "encoding": "json", "maxSupportedTransactionVersion": 1 }])).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("  skipped {}…: {e}", &sig[..8]);
                skipped += 1;
                continue;
            }
        };
        let decoded = res["slot"].as_u64().and_then(|slot| frame_from_rpc_json(sig, slot, &res).ok()).and_then(|f| decode_transaction(f, vec![]).ok());
        match decoded {
            Some(t) => txs.push(t),
            None => skipped += 1,
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    txs.sort_by_key(|t| (t.slot, t.index));
    println!("Decoded {} transactions ({skipped} could not be fetched or decoded).", txs.len());

    let db = std::env::temp_dir().join(format!("sentinel-verify-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let store = Arc::new(Store::open(db.to_str().unwrap())?);
    let (bus, _) = broadcast::channel(4);
    let rpc = Arc::new(solana_client::nonblocking::rpc_client::RpcClient::new(url.clone()));
    let s = Sentinel::new(store.clone(), Arc::new(Quiet(bus)), Some(rpc), sentinel::pricing::PriceBook::new(), "http://localhost".into())?;
    s.add_program(program.clone(), None)?;

    let idl = s.idls.get(&program).await;
    match &idl {
        Some(i) => println!("IDL on chain: {} ({} instructions, {} events)", i.name.as_deref().unwrap_or("?"), i.schema().len(), i.event_schema().len()),
        None => println!("No Anchor IDL on chain for this program."),
    }
    // Rules from the IDL's own suggestions that need no number.
    let mut rule_ids = Vec::new();
    if let Some(i) = &idl {
        for sg in sentinel::suggest::from_idl(i).into_iter().filter(|x| x.needs_value.is_none()) {
            let rule = store.create_rule(AlertRule {
                id: 0,
                owner: None,
                name: sg.title.clone(),
                program_id: Some(program.clone()),
                condition: serde_json::from_value(sg.condition.clone())?,
                create_incident: true,
                severity: sg.severity,
                webhook_url: None,
                channels: vec![],
                enabled: true,
                cooldown_secs: 0,
                created_at: Utc::now(),
                last_fired_at: None,
            })?;
            rule_ids.push((rule.id, sg.title));
        }
        s.reload_rules()?;
    }

    let mut instructions: BTreeMap<String, usize> = BTreeMap::new();
    let mut events: BTreeMap<String, usize> = BTreeMap::new();
    let (mut ok, mut failed) = (0, 0);
    for t in &txs {
        if t.success { ok += 1 } else { failed += 1 }
        for ix in t.instructions.iter().filter(|i| i.program_id == program) {
            *instructions.entry(ix.name.clone().unwrap_or_else(|| "(unnamed)".into())).or_default() += 1;
        }
        if let Some(i) = &idl {
            for e in sentinel::events::emitted(t, &program, i) {
                *events.entry(e.name).or_default() += 1;
            }
        }
        s.on_transaction(Arc::new(t.clone()));
    }
    s.flush();

    println!("\n{ok} succeeded, {failed} failed");
    println!("\nInstructions called:");
    for (n, c) in &instructions {
        println!("  {c:>4}  {n}");
    }
    println!("\nEvents decoded:");
    if events.is_empty() {
        println!("  none");
    }
    for (n, c) in &events {
        println!("  {c:>4}  {n}");
    }
    println!("\nIncidents opened by the suggested rules:");
    let incidents = store.incidents(None, 100)?;
    for (id, title) in &rule_ids {
        let n = incidents.iter().filter(|i| i.source == format!("rule:{id}")).count();
        println!("  {n:>4}  {title}");
    }
    let stored = s.recent_events(&program, None, 5);
    if let Some(e) = stored.first() {
        println!("\nLatest stored event: {} {}", e.name, e.fields.to_string().chars().take(200).collect::<String>());
    }
    let _ = std::fs::remove_file(&db);
    Ok(())
}
