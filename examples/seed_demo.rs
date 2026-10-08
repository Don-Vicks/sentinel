//! Fills a database with a day of realistic history so the dashboard, summaries,
//! post-mortems and MCP tools have something to show without waiting a day:
//! hourly rollups, an upgrade followed by a failure spike it caused, and a
//! vault outflow. Not for production data.
//!
//!     cargo run --example seed_demo -- demo.db
//!     SENTINEL_DB=demo.db SENTINEL_PROGRAMS=6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P cargo run

use chrono::{Duration, Utc};
use sentinel::model::{Incident, MonitoredProgram};
use sentinel::rollup::{BigMove, Observation, Rollup};
use sentinel::store::Store;
use serde_json::json;

const PUMP: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";

fn hour_rollup(h: i64, busy: f64, bad: f64) -> Rollup {
    let mut r = Rollup::default();
    let tx = (9_000.0 * busy) as u64;
    let buy = vec!["Buy".to_string()];
    let sell = vec!["Sell".to_string()];
    let create = vec!["Create".to_string()];
    for i in 0..tx {
        let ok = (i as f64 / tx as f64) >= bad;
        let names = match i % 10 {
            0..=5 => &buy,
            6..=8 => &sell,
            _ => &create,
        };
        let wallet = format!("wallet{}", (i * 7 + h as u64 * 131) % (2_800 + (h as u64 % 5) * 200));
        r.record(Observation {
            ok,
            fee: 5_000 + (i % 7) * 400,
            compute_units: Some(38_000 + (i * 31) % 22_000),
            signer: Some(&wallet),
            instructions: names,
            error: (!ok).then_some(("pump:Sell:TooLittleSolReceived", "TooLittleSolReceived in Pump.fun::Sell (#6003)")),
            usd: if ok { 41.0 } else { 0.0 },
            sol: if ok { 0.27 } else { 0.0 },
            big: None,
        });
    }
    r.note_tps(11.0 + busy * 9.0, 0);
    r
}

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| "demo.db".into());
    let store = Store::open(&path)?;
    store.upsert_program(&MonitoredProgram {
        program_id: PUMP.into(),
        label: "Pump.fun".into(),
        created_at: Utc::now() - Duration::days(2),
        detection: Default::default(),
        muted_until: None,
        mute_reason: None,
    })?;

    // Twenty-four hours of traffic, busier in the evening, with a bad patch after the upgrade.
    let now = Utc::now().timestamp();
    let this_hour = now - now % 3600;
    for back in 0..24 {
        let hour = this_hour - back * 3600;
        let busy = 0.8 + ((back as f64 / 24.0) * std::f64::consts::TAU).sin().abs() * 0.7;
        let bad = if back == 5 { 0.31 } else { 0.012 };
        let mut r = hour_rollup(hour / 3600, busy, bad);
        r.note_tps(r.peak_tps, hour + 1_800);
        if back == 7 {
            r.largest.push(BigMove { signature: "5Hq8DemoLargestTransferSignature111111111111111111111111111111111111111111111111".into(), at: hour + 600, amount: 1_250.0, symbol: "SOL".into(), usd: Some(212_500.0) });
        }
        if back == 2 {
            r.vault_net_usd = -412_000.0;
        }
        store.put_rollup(PUMP, hour, &r)?;
    }

    let upgrade_at = Utc::now() - Duration::hours(5) - Duration::minutes(4);
    let mk = |kind: &str, severity: &str, title: &str, summary: &str, explanation: &str, at, resolved_after: Option<i64>, evidence: serde_json::Value, wallets: i64, count: i64, metric: Option<&str>, observed: Option<f64>| -> anyhow::Result<Incident> {
        let incident: Incident = serde_json::from_value(json!({
            "id": 0, "program_id": PUMP, "kind": kind, "severity": severity, "status": if resolved_after.is_some() { "resolved" } else { "open" },
            "title": title, "summary": summary, "explanation": explanation, "source": "detector",
            "metric": metric, "observed": observed, "peak": observed, "baseline": null, "threshold": null,
            "onset_at": at, "detected_at": at, "updated_at": at,
            "resolved_at": resolved_after.map(|m| at + Duration::minutes(m)),
            "detection_latency_ms": 2600, "affected_count": count, "affected_wallets": wallets, "evidence": evidence,
        }))?;
        store.create_incident(incident)
    };

    let upgrade = mk(
        "authority_change", "high", "Program upgraded · Pump.fun",
        "New code deployed from buffer Bu77…erXy by 7Kq2…sQ9d (via CPI, e.g. a multisig vote)",
        "Transaction 3xFd…kP2a (slot 301222818) called the upgradeable loader on 6EF8…F6P. Whoever holds the upgrade authority can replace the program's code at any time, so every upgrade is worth confirming against your release process.",
        upgrade_at, Some(5),
        json!({ "authority": { "action": "upgrade", "buffer": "Bu77erXyDemoBuffer1111111111111111111111111", "program_id": PUMP, "programdata": "ProgDataDemo111111111111111111111111111111111", "authority": "7Kq2DemoVaultPda1111111111111111111111sQ9d", "signature": "3xFdDemoUpgradeSignature1111111111111111111111111111111111111111111111111111111kP2a", "slot": 301222818, "path": "0.1" } }),
        1, 1, Some("authority"), None,
    )?;
    let spike_at = upgrade_at + Duration::seconds(74);
    mk(
        "failure_spike", "high", "Transaction failure spike · Pump.fun",
        "31.2% of transactions failing (normally 1.2%)",
        "Failure rate over the last 60s was 31.2%, above the 3.0% threshold (2.5× the 1.2% baseline). This began 74s after the program was upgraded (transaction 3xFd…kP2a, incident #1001), so the new code is the first suspect.",
        spike_at, Some(14),
        json!({
            "fingerprints": [
                { "key": "pump:Sell:TooLittleSolReceived", "program_id": PUMP, "program_name": "Pump.fun", "instruction": "Sell", "error": "TooLittleSolReceived", "code": 6003, "count": 3180, "share": 0.93, "samples": [] },
                { "key": "pump:Buy:BuyZeroAmount", "program_id": PUMP, "program_name": "Pump.fun", "instruction": "Buy", "error": "BuyZeroAmount", "code": 6020, "count": 240, "share": 0.07, "samples": [] }
            ],
            "fingerprinted": 3420,
            "deploy": { "incident_id": upgrade.id, "signature": "3xFdDemoUpgradeSignature1111111111111111111111111111111111111111111111111111111kP2a", "at": upgrade_at, "slot": 301222818, "authority": "7Kq2DemoVaultPda1111111111111111111111sQ9d", "seconds_before": 74, "note": format!("This began 74s after the program was upgraded (transaction 3xFd…kP2a, incident #{}), so the new code is the first suspect.", upgrade.id) }
        }),
        1_460, 3_420, Some("failure_rate"), Some(31.2),
    )?;
    let drain_at = Utc::now() - Duration::hours(2) - Duration::minutes(11);
    mk(
        "vault_drain", "high", "Vault outflow · Pump.fun",
        "412,000 USDC ($412,000) left vault Vau1…c9Xq, 41% of its balance",
        "Net outflow from vault Vau1tDemo… was 412000.0000 USDC ($412,000) within 10 minutes, leaving 593000.0000. That is 41.0% of what it held, against the 20% threshold or $100,000.",
        drain_at, Some(22),
        json!({ "vault": { "account": "Vau1tDemoTreasuryTokenAccount11111111111111c9Xq", "mint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", "symbol": "USDC", "outflow": 412000.0, "balance_after": 593000.0, "pct": 41.0, "usd": 412000.0 } }),
        3, 9, Some("vault_outflow_pct"), Some(41.0),
    )?;
    mk(
        "error_spike", "medium", "New error · Pump.fun",
        "AccountNotInitialized appeared 14 times in 60s (never seen before)",
        "A new error type appeared: AccountNotInitialized in Pump.fun::Buy (#3012) occurred 14 times in the last 60s and had never been seen for this program.",
        Utc::now() - Duration::minutes(9), None,
        json!({ "fingerprints": [{ "key": "pump:Buy:AccountNotInitialized", "program_id": PUMP, "program_name": "Pump.fun", "instruction": "Buy", "error": "AccountNotInitialized", "code": 3012, "count": 14, "share": 1.0, "samples": [] }], "fingerprinted": 14 }),
        11, 14, Some("error_count"), Some(14.0),
    )?;
    println!("Seeded {path}: 24 hours of rollups and 4 incidents for Pump.fun ({PUMP}).");
    Ok(())
}
