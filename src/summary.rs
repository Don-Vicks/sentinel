//! "What did this program do?" over a period: activity, value moved, behaviour,
//! reliability and security. Built from the stored hourly rollups and the
//! incident history, so it covers more than the in-memory window and survives
//! restarts. Pure arithmetic over stored data: no estimates beyond the wallet
//! count (a HyperLogLog, about 3% off).

use crate::model::{Incident, IncidentKind, IncidentStatus, Severity};
use crate::rollup::{BigMove, Rollup};
use crate::store::Store;
use anyhow::Result;
use serde::Serialize;

const HOUR: i64 = 3600;
const TOP: usize = 5;

#[derive(Debug, Clone, Default, Serialize)]
pub struct Activity {
    pub tx: u64,
    pub failed: u64,
    /// Percent of transactions that succeeded; `None` when there were none.
    pub success_rate: Option<f64>,
    pub unique_wallets: u64,
    pub peak_tps: f64,
    pub peak_at: Option<i64>,
    pub fees_sol: f64,
    pub avg_compute: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Value {
    /// Value moved by priced, liquid tokens (Solami Blur) and SOL. A transaction with several
    /// hops counts each hop; tokens under the liquidity floor are not valued.
    pub usd_volume: f64,
    pub sol_volume: f64,
    /// Net USD change across watched vaults; `None` when none are watched or none could be priced.
    pub vault_net_usd: Option<f64>,
    pub largest: Vec<BigMove>,
}

#[derive(Debug, Clone, Serialize)]
pub struct IxRow {
    pub name: String,
    pub tx: u64,
    pub share: f64,
    pub failure_rate: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrRow {
    pub label: String,
    pub count: u64,
    /// Share of all failed transactions.
    pub share: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct IncidentRef {
    pub id: i64,
    pub kind: IncidentKind,
    pub severity: Severity,
    pub status: IncidentStatus,
    pub title: String,
    pub summary: String,
    pub detected_at: i64,
    pub resolved_at: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Reliability {
    pub opened: usize,
    pub resolved: usize,
    pub open_now: usize,
    /// Minutes covered by at least one reliability incident (spikes, stopped activity, rules).
    pub minutes_in_incident: u64,
    /// Mean time to detect: seconds from the first affected transaction to the incident.
    pub mttd_secs: Option<f64>,
    /// Mean time to resolve, from detection.
    pub mttr_secs: Option<f64>,
    pub worst: Option<IncidentRef>,
    pub incidents: Vec<IncidentRef>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HourPoint {
    pub hour: i64,
    pub tx: u64,
    pub failed: u64,
    pub usd_volume: f64,
    /// Average and lowest health score sampled in the hour.
    pub health_avg: Option<f64>,
    pub health_min: Option<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub program_id: String,
    pub label: String,
    pub from: i64,
    pub to: i64,
    pub period_secs: i64,
    pub headline: String,
    pub activity: Activity,
    pub previous: Activity,
    pub previous_usd_volume: f64,
    pub value: Value,
    pub top_instructions: Vec<IxRow>,
    pub top_errors: Vec<ErrRow>,
    pub busiest_hour: Option<HourPoint>,
    pub hourly: Vec<HourPoint>,
    pub reliability: Reliability,
    /// Upgrades, authority changes and closures in the period.
    pub program_changes: Vec<IncidentRef>,
    /// Things worth doing, from what the data shows.
    pub next_actions: Vec<String>,
    /// How much of the period has data, 0 to 1. Less than 1 means Sentinel wasn't watching all of it.
    pub coverage: f64,
}

fn activity(rollup: &Rollup) -> Activity {
    Activity {
        tx: rollup.tx,
        failed: rollup.failed,
        success_rate: (rollup.tx > 0).then(|| (rollup.tx - rollup.failed) as f64 * 100.0 / rollup.tx as f64),
        unique_wallets: rollup.signers.count(),
        peak_tps: rollup.peak_tps,
        peak_at: (rollup.peak_tps > 0.0).then_some(rollup.peak_at),
        fees_sol: rollup.fees as f64 / 1e9,
        avg_compute: (rollup.cu_n > 0).then(|| rollup.cu_sum as f64 / rollup.cu_n as f64),
    }
}

fn merged(rows: &[(i64, Rollup)]) -> Rollup {
    let mut all = Rollup::default();
    for (_, r) in rows {
        all.merge(r);
    }
    all
}

fn incident_ref(i: &Incident) -> IncidentRef {
    IncidentRef {
        id: i.id,
        kind: i.kind,
        severity: i.severity,
        status: i.status,
        title: i.title.clone(),
        summary: i.summary.clone(),
        detected_at: i.detected_at.timestamp(),
        resolved_at: i.resolved_at.map(|t| t.timestamp()),
    }
}

/// Kinds that mean the program was unhealthy, as opposed to something happening to it.
fn is_reliability(kind: IncidentKind) -> bool {
    !matches!(
        kind,
        IncidentKind::AuthorityChange | IncidentKind::DependencyChange | IncidentKind::LargeTransfer | IncidentKind::BotActivity
    )
}

/// Total seconds covered by the union of `[start, end)` intervals.
fn union_secs(mut spans: Vec<(i64, i64)>) -> i64 {
    spans.sort_unstable();
    let mut total = 0;
    let mut current: Option<(i64, i64)> = None;
    for (s, e) in spans {
        match current {
            Some((cs, ce)) if s <= ce => current = Some((cs, ce.max(e))),
            Some((cs, ce)) => {
                total += ce - cs;
                current = Some((s, e));
            }
            None => current = Some((s, e)),
        }
    }
    if let Some((cs, ce)) = current {
        total += ce - cs;
    }
    total
}

fn reliability(incidents: &[Incident], from: i64, to: i64) -> Reliability {
    let in_period: Vec<&Incident> = incidents
        .iter()
        .filter(|i| i.detected_at.timestamp() >= from && i.detected_at.timestamp() < to && is_reliability(i.kind))
        .collect();
    let latencies: Vec<f64> = in_period.iter().filter_map(|i| i.detection_latency_ms).map(|ms| ms as f64 / 1000.0).collect();
    let repairs: Vec<f64> = in_period
        .iter()
        .filter_map(|i| i.resolved_at.map(|r| (r - i.detected_at).num_seconds().max(0) as f64))
        .collect();
    let mean = |v: &[f64]| (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64);
    let spans = in_period
        .iter()
        .map(|i| {
            let start = i.onset_at.unwrap_or(i.detected_at).timestamp().max(from);
            let end = i.resolved_at.map(|r| r.timestamp()).unwrap_or(to).min(to);
            (start, end.max(start))
        })
        .collect();
    let worst = in_period
        .iter()
        .max_by_key(|i| (i.severity, i.affected_count))
        .map(|i| incident_ref(i));
    Reliability {
        opened: in_period.len(),
        resolved: in_period.iter().filter(|i| i.status == IncidentStatus::Resolved).count(),
        open_now: incidents.iter().filter(|i| i.status != IncidentStatus::Resolved && is_reliability(i.kind)).count(),
        minutes_in_incident: (union_secs(spans) as f64 / 60.0).round() as u64,
        mttd_secs: mean(&latencies),
        mttr_secs: mean(&repairs),
        worst,
        incidents: in_period.iter().take(10).map(|i| incident_ref(i)).collect(),
    }
}

pub fn count(n: u64) -> String {
    match n {
        0..=9_999 => group(n),
        10_000..=999_999 => format!("{:.1}K", n as f64 / 1e3),
        1_000_000..=999_999_999 => format!("{:.2}M", n as f64 / 1e6),
        _ => format!("{:.2}B", n as f64 / 1e9),
    }
}

fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn usd(v: f64) -> String {
    match v {
        v if v >= 1e9 => format!("${:.2}B", v / 1e9),
        v if v >= 1e6 => format!("${:.2}M", v / 1e6),
        v if v >= 1e4 => format!("${:.1}K", v / 1e3),
        v => format!("${}", group(v.round() as u64)),
    }
}

fn period_name(secs: i64) -> String {
    match secs {
        s if s == HOUR => "hour".into(),
        s if s == 24 * HOUR => "24 hours".into(),
        s if s == 7 * 24 * HOUR => "7 days".into(),
        s if s % (24 * HOUR) == 0 => format!("{} days", s / (24 * HOUR)),
        s => format!("{} hours", s / HOUR),
    }
}

/// Builds the summary for `[now - period_secs, now)`.
pub fn build(store: &Store, program_id: &str, label: &str, now: i64, period_secs: i64, idl_loaded: bool) -> Result<Summary> {
    let period_secs = period_secs.clamp(HOUR, 30 * 24 * HOUR);
    let from = now - period_secs;
    let from_hour = from - from.rem_euclid(HOUR);
    let rows = store.rollups_between(program_id, from_hour, now)?;
    let prev_rows = store.rollups_between(program_id, from_hour - period_secs, from_hour)?;
    let all = merged(&rows);
    let prev = merged(&prev_rows);
    let activity = activity(&all);

    let top_instructions = {
        let mut v: Vec<_> = all.instructions.iter().collect();
        v.sort_by(|a, b| b.1.tx.cmp(&a.1.tx).then(a.0.cmp(b.0)));
        v.into_iter()
            .take(TOP)
            .map(|(name, c)| IxRow {
                name: name.clone(),
                tx: c.tx,
                share: if all.tx > 0 { c.tx as f64 / all.tx as f64 } else { 0.0 },
                failure_rate: if c.tx > 0 { c.failed as f64 * 100.0 / c.tx as f64 } else { 0.0 },
            })
            .collect::<Vec<_>>()
    };
    let top_errors = {
        let mut v: Vec<_> = all.errors.values().collect();
        v.sort_by(|a, b| b.count.cmp(&a.count).then(a.label.cmp(&b.label)));
        v.into_iter()
            .take(TOP)
            .map(|e| ErrRow {
                label: e.label.clone(),
                count: e.count,
                share: if all.failed > 0 { e.count as f64 / all.failed as f64 } else { 0.0 },
            })
            .collect::<Vec<_>>()
    };
    let hourly: Vec<HourPoint> = rows
        .iter()
        .map(|(h, r)| HourPoint {
            hour: *h,
            tx: r.tx,
            failed: r.failed,
            usd_volume: r.usd_volume,
            health_avg: (r.health_n > 0).then(|| r.health_sum as f64 / r.health_n as f64),
            health_min: r.health_min,
        })
        .collect();
    let busiest_hour = hourly.iter().filter(|h| h.tx > 0).max_by_key(|h| h.tx).cloned();

    let incidents = store.incidents(Some(program_id), 1000)?;
    let reliability = reliability(&incidents, from, now);
    let program_changes: Vec<IncidentRef> = incidents
        .iter()
        .filter(|i| matches!(i.kind, IncidentKind::AuthorityChange | IncidentKind::DependencyChange) && i.detected_at.timestamp() >= from && i.detected_at.timestamp() < now)
        .map(incident_ref)
        .collect();

    let covered = rows.iter().filter(|(_, r)| r.tx > 0).count() as f64;
    let expected = ((now - from_hour) as f64 / HOUR as f64).ceil().max(1.0);
    let coverage = (covered / expected).min(1.0);

    let mut next_actions = Vec::new();
    if !idl_loaded {
        next_actions.push("Load the program's Anchor IDL so instructions, arguments and custom errors are named.".to_string());
    }
    if let Some(top) = top_errors.first().filter(|e| e.share >= 0.5 && all.failed >= 20) {
        next_actions.push(format!("{:.0}% of failures are {}; start there.", top.share * 100.0, top.label));
    }
    if reliability.open_now > 0 {
        next_actions.push(format!("{} incident{} still open.", reliability.open_now, if reliability.open_now == 1 { " is" } else { "s are" }));
    }
    if !program_changes.is_empty() {
        next_actions.push(format!(
            "Confirm the {} program change{} {} your release process.",
            program_changes.len(),
            if program_changes.len() == 1 { "" } else { "s" },
            if program_changes.len() == 1 { "matches" } else { "match" }
        ));
    }
    if coverage < 0.9 && all.tx > 0 {
        next_actions.push(format!("Sentinel only saw traffic in {:.0}% of this period's hours, so totals may be low.", coverage * 100.0));
    }

    let headline = if all.tx == 0 {
        format!("{label} had no observed transactions in the last {}.", period_name(period_secs))
    } else {
        format!(
            "{label} handled {} transactions in the last {} ({:.1}% succeeded){}. {}",
            count(all.tx),
            period_name(period_secs),
            activity.success_rate.unwrap_or(0.0),
            if all.usd_volume > 0.0 { format!(", moving {}", usd(all.usd_volume)) } else { String::new() },
            match (reliability.opened, reliability.open_now) {
                (0, _) => "No incidents.".to_string(),
                (n, 0) => format!("{n} incident{}, all resolved.", if n == 1 { "" } else { "s" }),
                (n, open) => format!("{n} incident{}, {open} still open.", if n == 1 { "" } else { "s" }),
            }
        )
    };

    Ok(Summary {
        program_id: program_id.to_string(),
        label: label.to_string(),
        from,
        to: now,
        period_secs,
        headline,
        activity,
        previous: activity_of(&prev),
        previous_usd_volume: prev.usd_volume,
        value: Value {
            usd_volume: all.usd_volume,
            sol_volume: all.sol_volume,
            vault_net_usd: (all.vault_net_usd != 0.0).then_some(all.vault_net_usd),
            largest: all.largest.clone(),
        },
        top_instructions,
        top_errors,
        busiest_hour,
        hourly,
        reliability,
        program_changes,
        next_actions,
        coverage,
    })
}

fn activity_of(r: &Rollup) -> Activity {
    activity(r)
}

fn change(now: f64, before: f64) -> Option<String> {
    if before <= 0.0 {
        return None;
    }
    let pct = (now - before) * 100.0 / before;
    Some(format!("{}{:.0}%", if pct >= 0.0 { "+" } else { "" }, pct))
}

fn clock(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0).map(|t| t.format("%H:%M UTC").to_string()).unwrap_or_default()
}

fn duration(secs: f64) -> String {
    match secs {
        s if s < 90.0 => format!("{s:.0}s"),
        s if s < 5400.0 => format!("{:.0}m", s / 60.0),
        s => format!("{:.1}h", s / 3600.0),
    }
}

impl Summary {
    /// Label/value rows for chat messages. Fewer rows when there was nothing to report.
    pub fn facts(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let a = &self.activity;
        if a.tx == 0 {
            return out;
        }
        let vs = change(a.tx as f64, self.previous.tx as f64)
            .map(|c| format!(" ({c} vs previous)"))
            .unwrap_or_default();
        out.push(("Transactions".into(), format!("{}{vs}", count(a.tx))));
        out.push((
            "Success rate".into(),
            format!("{:.2}% · {} failed", a.success_rate.unwrap_or(0.0), count(a.failed)),
        ));
        out.push(("Unique wallets".into(), format!("~{}", count(a.unique_wallets))));
        if self.value.usd_volume > 0.0 || self.value.sol_volume > 0.0 {
            let mut v = Vec::new();
            if self.value.usd_volume > 0.0 {
                v.push(usd(self.value.usd_volume));
            }
            if self.value.sol_volume > 0.0 {
                v.push(format!("{} SOL", count(self.value.sol_volume.round() as u64)));
            }
            let vs = change(self.value.usd_volume, self.previous_usd_volume)
                .map(|c| format!(" ({c})"))
                .unwrap_or_default();
            out.push(("Value moved".into(), format!("{}{vs}", v.join(" · "))));
        }
        if let Some(net) = self.value.vault_net_usd {
            out.push((
                "Vault balance".into(),
                format!("{} {} across watched vaults", if net >= 0.0 { "up" } else { "down" }, usd(net.abs())),
            ));
        }
        if let Some(big) = self.value.largest.first() {
            out.push((
                "Largest transfer".into(),
                format!("{} {}{}", count(big.amount.round() as u64), big.symbol, big.usd.map(|u| format!(" ({})", usd(u))).unwrap_or_default()),
            ));
        }
        if let Some(at) = a.peak_at {
            out.push(("Peak".into(), format!("{:.0} TPS at {}", a.peak_tps, clock(at))));
        }
        if !self.top_instructions.is_empty() {
            out.push((
                "Top instructions".into(),
                self.top_instructions
                    .iter()
                    .take(3)
                    .map(|i| format!("{} {:.0}%", i.name, i.share * 100.0))
                    .collect::<Vec<_>>()
                    .join(" · "),
            ));
        }
        if let Some(e) = self.top_errors.first() {
            out.push(("Top error".into(), format!("{} ({}×)", e.label, count(e.count))));
        }
        let r = &self.reliability;
        out.push((
            "Incidents".into(),
            if r.opened == 0 {
                "none".into()
            } else {
                let mut s = format!("{} opened, {} resolved", r.opened, r.resolved);
                if let Some(m) = r.mttd_secs {
                    s.push_str(&format!(" · detected in {}", duration(m)));
                }
                if let Some(m) = r.mttr_secs {
                    s.push_str(&format!(" · resolved in {}", duration(m)));
                }
                s
            },
        ));
        if !self.program_changes.is_empty() {
            out.push((
                "Program changes".into(),
                self.program_changes.iter().take(3).map(|c| c.summary.clone()).collect::<Vec<_>>().join("; "),
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rollup::Observation;

    fn store() -> Store {
        let path = std::env::temp_dir().join(format!("sentinel-summary-{}-{:?}.db", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_file(&path);
        Store::open(path.to_str().unwrap()).unwrap()
    }

    fn hour(tx: u64, failed: u64) -> Rollup {
        let mut r = Rollup::default();
        let buy = vec!["Buy".to_string()];
        let sell = vec!["Sell".to_string()];
        for i in 0..tx {
            let ok = i >= failed;
            let names = if i % 4 == 0 { &sell } else { &buy };
            r.record(Observation {
                ok,
                fee: 5000,
                compute_units: Some(40_000),
                signer: Some(&format!("w{}", i % 20)),
                instructions: names,
                error: (!ok).then_some(("k", "TooLittleSolReceived in Pump.fun::Sell (#6003)")),
                usd: 10.0,
                sol: 0.1,
                big: None,
            });
        }
        r.note_tps(12.0, 1000);
        r
    }

    #[test]
    fn totals_deltas_and_top_lists() {
        let s = store();
        let now = 1_700_000_000 - 1_700_000_000 % 3600 + 1800; // half past an hour
        let this_hour = now - now % 3600;
        // Previous day: 100 tx. Today, three hours: 150 tx with 30 failing.
        s.put_rollup("P", this_hour - 25 * 3600, &hour(100, 0)).unwrap();
        for (i, (tx, failed)) in [(50, 10), (50, 10), (50, 10)].into_iter().enumerate() {
            s.put_rollup("P", this_hour - i as i64 * 3600, &hour(tx, failed)).unwrap();
        }
        let sum = build(&s, "P", "Pump.fun", now, 24 * 3600, true).unwrap();
        assert_eq!(sum.activity.tx, 150);
        assert_eq!(sum.activity.failed, 30);
        assert!((sum.activity.success_rate.unwrap() - 80.0).abs() < 1e-9);
        assert_eq!(sum.previous.tx, 100);
        assert!((sum.value.usd_volume - 1500.0).abs() < 1e-6);
        assert_eq!(sum.top_instructions[0].name, "Buy");
        assert_eq!(sum.top_errors[0].count, 30);
        assert!((sum.top_errors[0].share - 1.0).abs() < 1e-9);
        assert_eq!(sum.busiest_hour.as_ref().unwrap().tx, 50);
        assert!(sum.headline.contains("150 transactions") && sum.headline.contains("80.0%"), "{}", sum.headline);
        let facts = sum.facts();
        let get = |k: &str| facts.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap_or_default();
        assert!(get("Transactions").contains("+50% vs previous"), "{}", get("Transactions"));
        assert!(get("Value moved").starts_with("$1,500"), "{}", get("Value moved"));
        assert_eq!(get("Incidents"), "none");
        assert!(sum.next_actions.iter().any(|a| a.contains("100% of failures")), "{:?}", sum.next_actions);
        assert!(sum.coverage < 0.2, "only 3 of 25 hours have data: {}", sum.coverage);
    }

    #[test]
    fn empty_period_and_missing_idl_say_so() {
        let s = store();
        let sum = build(&s, "nothing", "Quiet", 1_700_000_000, 24 * 3600, false).unwrap();
        assert_eq!(sum.activity.tx, 0);
        assert!(sum.headline.contains("no observed transactions"));
        assert!(sum.facts().is_empty());
        assert!(sum.next_actions.iter().any(|a| a.contains("IDL")));
    }

    #[test]
    fn overlapping_incidents_count_once() {
        assert_eq!(union_secs(vec![(0, 100), (50, 150), (300, 360)]), 210);
        assert_eq!(union_secs(vec![]), 0);
        assert_eq!(count(1_234_567), "1.23M");
        assert_eq!(count(9_999), "9,999");
        assert_eq!(usd(4_100_000.0), "$4.10M");
    }
}
