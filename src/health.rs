//! "Is this program OK right now?" A 0-100 score built from separate checks,
//! each stating the rule it applied and the numbers it saw. A check that has
//! nothing to judge (a program still warming up, no IDL needed) is marked
//! unknown and left out of the score instead of passing by default.

use crate::live::ProgramSnapshot;
use crate::model::{Incident, Severity};
use crate::posture::Posture;
use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pass,
    Warn,
    Fail,
    /// Nothing to judge yet; not part of the score.
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub id: &'static str,
    pub label: &'static str,
    pub status: Status,
    /// 0-100, or `None` when unknown.
    pub score: Option<u8>,
    /// How much this check counts toward the overall score.
    pub weight: u32,
    /// The rule applied and the numbers behind it.
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Health {
    pub program_id: String,
    /// `None` while there is nothing to judge.
    pub score: Option<u8>,
    /// "healthy", "degraded", "critical" or "learning".
    pub status: &'static str,
    pub headline: String,
    pub checks: Vec<Check>,
}

pub struct Inputs<'a> {
    pub snapshot: &'a ProgramSnapshot,
    /// Incidents that are not resolved.
    pub open: &'a [Incident],
    pub feed_stalled: bool,
    pub idl_loaded: bool,
    pub posture: Option<&'a Posture>,
    /// How many vault accounts are being watched for drains.
    pub vaults_watched: usize,
    /// Programs this one has been seen calling.
    pub dependencies: usize,
    pub now: i64,
}

fn check(id: &'static str, label: &'static str, weight: u32, status: Status, score: Option<u8>, detail: String) -> Check {
    Check { id, label, status, score, weight, detail }
}

fn unknown(id: &'static str, label: &'static str, weight: u32, detail: String) -> Check {
    check(id, label, weight, Status::Unknown, None, detail)
}

fn graded(id: &'static str, label: &'static str, weight: u32, score: f64, detail: String) -> Check {
    let score = score.clamp(0.0, 100.0).round() as u8;
    let status = match score {
        85..=100 => Status::Pass,
        50..=84 => Status::Warn,
        _ => Status::Fail,
    };
    check(id, label, weight, status, Some(score), detail)
}

fn reliability(s: &ProgramSnapshot) -> Check {
    const MIN_TX: u64 = 20;
    if matches!(s.health, "warming_up") {
        return unknown("reliability", "Failure rate", 35, format!("Learning this program's normal behaviour ({}s left).", s.warmup_remaining_secs));
    }
    if s.tx_60s < MIN_TX {
        return unknown(
            "reliability",
            "Failure rate",
            35,
            format!("{} transactions in the last minute; at least {MIN_TX} are needed to judge a rate.", s.tx_60s),
        );
    }
    // Tolerate the larger of 2 points or half again the normal rate, then lose 100 points by 30 points over.
    let excess = s.failure_rate_60s - s.baseline_failure_rate;
    let slack = (s.baseline_failure_rate * 0.5).max(2.0);
    let score = if excess <= slack { 100.0 } else { 100.0 - (excess - slack) / (30.0 - slack).max(1.0) * 100.0 };
    graded(
        "reliability",
        "Failure rate",
        35,
        score,
        format!(
            "{:.1}% of {} transactions failed in the last minute (normal {:.1}%; fine within {:.1} points of normal).",
            s.failure_rate_60s, s.tx_60s, s.baseline_failure_rate, slack
        ),
    )
}

fn incidents(open: &[Incident]) -> Check {
    if open.is_empty() {
        return check("incidents", "Open incidents", 30, Status::Pass, Some(100), "No open incidents.".into());
    }
    let weight = |s: Severity| match s {
        Severity::Info => 0.0,
        Severity::Low => 5.0,
        Severity::Medium => 20.0,
        Severity::High => 40.0,
        Severity::Critical => 70.0,
    };
    let penalty: f64 = open.iter().map(|i| weight(i.severity)).sum();
    let count = |sev: Severity| open.iter().filter(|i| i.severity == sev).count();
    let parts: Vec<String> = [(Severity::Critical, "critical"), (Severity::High, "high"), (Severity::Medium, "medium"), (Severity::Low, "low"), (Severity::Info, "info")]
        .into_iter()
        .filter(|(s, _)| count(*s) > 0)
        .map(|(s, n)| format!("{} {n}", count(s)))
        .collect();
    graded(
        "incidents",
        "Open incidents",
        30,
        100.0 - penalty,
        format!("{} open ({}). Each costs 5 (low), 20 (medium), 40 (high) or 70 (critical) points.", open.len(), parts.join(", ")),
    )
}

fn liveness(i: &Inputs) -> Check {
    let s = i.snapshot;
    if i.feed_stalled {
        return check(
            "liveness",
            "Transactions arriving",
            15,
            Status::Warn,
            Some(50),
            "Sentinel's own feed has stalled, so this program can't be judged right now. Detectors are paused.".into(),
        );
    }
    if s.baseline_tps < 0.05 || s.health == "warming_up" {
        return unknown("liveness", "Transactions arriving", 15, "Too little regular traffic to tell silence from a quiet period.".into());
    }
    let Some(last) = s.last_tx_at else {
        return unknown("liveness", "Transactions arriving", 15, "No transactions seen yet.".into());
    };
    let silent = (i.now - last.timestamp()).max(0);
    let usual_gap = 1.0 / s.baseline_tps;
    let limit = (usual_gap * 20.0).max(30.0);
    if (silent as f64) > limit {
        check(
            "liveness",
            "Transactions arriving",
            15,
            Status::Fail,
            Some(0),
            format!("No transactions for {silent}s; normally one every {usual_gap:.1}s (alarm after {limit:.0}s)."),
        )
    } else {
        check(
            "liveness",
            "Transactions arriving",
            15,
            Status::Pass,
            Some(100),
            format!("Last transaction {silent}s ago; normally one every {usual_gap:.1}s."),
        )
    }
}

fn compute(s: &ProgramSnapshot) -> Check {
    if s.tx_60s < 10 || s.baseline_avg_cu <= 0.0 || s.avg_cu_60s <= 0.0 || s.health == "warming_up" {
        return unknown("compute", "Compute usage", 10, "Not enough transactions to compare compute use with normal.".into());
    }
    let ratio = s.avg_cu_60s / s.baseline_avg_cu;
    let score = if ratio <= 1.5 { 100.0 } else { 100.0 - (ratio - 1.5) / 1.5 * 80.0 };
    graded(
        "compute",
        "Compute usage",
        10,
        score,
        format!("Average {:.0} compute units per transaction, {ratio:.1}× normal ({:.0}). Fine up to 1.5×.", s.avg_cu_60s, s.baseline_avg_cu),
    )
}

fn coverage(idl_loaded: bool) -> Check {
    if idl_loaded {
        check("coverage", "Decoding coverage", 5, Status::Pass, Some(100), "Anchor IDL loaded: instructions, arguments and custom errors are named.".into())
    } else {
        check(
            "coverage",
            "Decoding coverage",
            5,
            Status::Warn,
            Some(60),
            "No Anchor IDL for this program, so instructions and custom errors show as raw numbers.".into(),
        )
    }
}

fn dependencies(i: &Inputs) -> Check {
    if i.dependencies == 0 {
        return unknown("dependencies", "Dependencies", 5, "No programs called yet, or none worth tracking.".into());
    }
    let changed = i.open.iter().filter(|inc| inc.kind == crate::model::IncidentKind::DependencyChange).count();
    if changed > 0 {
        check(
            "dependencies",
            "Dependencies",
            5,
            Status::Warn,
            Some(60),
            format!("{changed} program{} this one calls changed recently. If anything fails, look there first.", if changed == 1 { "" } else { "s" }),
        )
    } else {
        check(
            "dependencies",
            "Dependencies",
            5,
            Status::Pass,
            Some(100),
            format!("Calls {} program{}; none has been upgraded recently.", i.dependencies, if i.dependencies == 1 { "" } else { "s" }),
        )
    }
}

fn funds(i: &Inputs) -> Check {
    if i.vaults_watched == 0 {
        return unknown(
            "funds",
            "Funds",
            10,
            "No vaults watched. Add the program's treasury or vault accounts to be alerted to drains.".into(),
        );
    }
    let draining = i.open.iter().filter(|inc| inc.kind == crate::model::IncidentKind::VaultDrain).count();
    if draining > 0 {
        check(
            "funds",
            "Funds",
            10,
            Status::Fail,
            Some(0),
            format!("{draining} watched vault{} losing funds right now.", if draining == 1 { " is" } else { "s are" }),
        )
    } else {
        check(
            "funds",
            "Funds",
            10,
            Status::Pass,
            Some(100),
            format!("Watching {} vault{}; no unusual outflow.", i.vaults_watched, if i.vaults_watched == 1 { "" } else { "s" }),
        )
    }
}

fn authority(p: Option<&Posture>) -> Check {
    let Some(p) = p else {
        return unknown("authority", "Upgrade authority", 5, "Not read from chain yet.".into());
    };
    match p.authority_kind {
        "single_key" => check(
            "authority",
            "Upgrade authority",
            5,
            Status::Warn,
            Some(40),
            "One wallet can replace this program's code. Move the upgrade authority to a multisig.".into(),
        ),
        "none" => check("authority", "Upgrade authority", 5, Status::Pass, Some(100), "Immutable: the code can't change.".into()),
        _ => check("authority", "Upgrade authority", 5, Status::Pass, Some(100), "Upgrades are controlled by a program (multisig or DAO).".into()),
    }
}

pub fn evaluate(i: &Inputs) -> Health {
    let checks = vec![
        reliability(i.snapshot),
        incidents(i.open),
        liveness(i),
        compute(i.snapshot),
        funds(i),
        dependencies(i),
        coverage(i.idl_loaded),
        authority(i.posture),
    ];
    let scored: Vec<&Check> = checks.iter().filter(|c| c.score.is_some()).collect();
    let weight: u32 = scored.iter().map(|c| c.weight).sum();
    // Coverage, authority and "no open incidents" alone say nothing about whether the program works.
    let judged = scored.iter().any(|c| matches!(c.id, "reliability" | "liveness" | "compute")) || !i.open.is_empty();
    let score = (judged && weight > 0).then(|| {
        let total: f64 = scored.iter().map(|c| c.score.unwrap_or(0) as f64 * c.weight as f64).sum();
        (total / weight as f64).round() as u8
    });
    let (status, headline) = match score {
        None => ("learning", "Not enough data yet to judge this program.".to_string()),
        Some(s) => {
            let worst = scored.iter().filter(|c| c.status == Status::Fail || c.status == Status::Warn).min_by_key(|c| c.score.unwrap_or(100));
            let status = match s {
                85..=100 => "healthy",
                60..=84 => "degraded",
                _ => "critical",
            };
            let headline = match (status, worst) {
                ("healthy", _) => "Everything Sentinel checks looks normal.".to_string(),
                (_, Some(w)) => format!("{}: {}", w.label, w.detail),
                _ => "Some checks are below normal.".to_string(),
            };
            (status, headline)
        }
    };
    Health { program_id: i.snapshot.program_id.clone(), score, status, headline, checks }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn snapshot() -> ProgramSnapshot {
        ProgramSnapshot {
            program_id: "P".into(),
            label: "Pump.fun".into(),
            health: "healthy",
            at: Utc::now(),
            warmup_remaining_secs: 0,
            total_tx: 10_000,
            total_failed: 200,
            tx_60s: 300,
            failed_60s: 6,
            failure_rate_60s: 2.0,
            tps_10s: 5.0,
            tps_60s: 5.0,
            avg_cu_60s: 40_000.0,
            max_cu_60s: 90_000,
            unique_signers_60s: 40,
            fees_60s_sol: 0.1,
            baseline_failure_rate: 2.0,
            baseline_tps: 5.0,
            baseline_avg_cu: 40_000.0,
            top_errors: vec![],
            instructions: vec![],
            open_incidents: 0,
            last_tx_at: Some(Utc.timestamp_opt(1_000, 0).unwrap()),
            point: None,
        }
    }

    fn inputs<'a>(s: &'a ProgramSnapshot, open: &'a [Incident]) -> Inputs<'a> {
        Inputs { snapshot: s, open, feed_stalled: false, idl_loaded: true, posture: None, vaults_watched: 0, dependencies: 0, now: 1_002 }
    }

    fn incident(sev: Severity) -> Incident {
        serde_json::from_value(serde_json::json!({
            "id": 1, "program_id": "P", "kind": "failure_spike", "severity": sev, "status": "open",
            "title": "t", "summary": "s", "explanation": "e", "source": "detector",
            "metric": null, "observed": null, "peak": null, "baseline": null, "threshold": null,
            "onset_at": null, "detected_at": "2024-01-01T00:00:00Z", "updated_at": "2024-01-01T00:00:00Z",
            "resolved_at": null, "detection_latency_ms": null, "affected_count": 0, "affected_wallets": 0,
            "evidence": {}
        }))
        .unwrap()
    }

    #[test]
    fn a_normal_program_is_healthy() {
        let s = snapshot();
        let h = evaluate(&inputs(&s, &[]));
        assert_eq!(h.status, "healthy", "{h:?}");
        assert!(h.score.unwrap() >= 95);
        assert_eq!(h.checks.len(), 8);
        // Authority wasn't read, so it is unknown and left out rather than passed.
        let a = h.checks.iter().find(|c| c.id == "authority").unwrap();
        assert_eq!((a.status, a.score), (Status::Unknown, None));
    }

    #[test]
    fn a_failure_spike_and_a_critical_incident_name_themselves() {
        let mut s = snapshot();
        s.failure_rate_60s = 45.0;
        s.failed_60s = 135;
        let open = [incident(Severity::Critical)];
        let h = evaluate(&inputs(&s, &open));
        assert_eq!(h.status, "critical", "{h:?}");
        let rel = h.checks.iter().find(|c| c.id == "reliability").unwrap();
        assert_eq!(rel.status, Status::Fail);
        assert!(rel.detail.contains("45.0%") && rel.detail.contains("normal 2.0%"), "{}", rel.detail);
        assert!(h.headline.contains("Failure rate") || h.headline.contains("Open incidents"), "{}", h.headline);
    }

    #[test]
    fn an_info_incident_is_listed_but_costs_nothing() {
        let s = snapshot();
        let calm = evaluate(&inputs(&s, &[])).score.unwrap();
        let open = [incident(Severity::Info)];
        let h = evaluate(&inputs(&s, &open));
        assert_eq!(h.score.unwrap(), calm, "{h:?}");
        assert!(Severity::Info < Severity::Low && Severity::Low < Severity::Medium);
        let worse = [incident(Severity::Low)];
        assert!(evaluate(&inputs(&s, &worse)).score.unwrap() < calm);
    }

    #[test]
    fn silence_is_a_failure_but_a_stalled_feed_is_not_the_programs_fault() {
        let s = snapshot();
        let mut i = inputs(&s, &[]);
        i.now = 1_000 + 600;
        let live = evaluate(&i).checks.into_iter().find(|c| c.id == "liveness").unwrap();
        assert_eq!(live.status, Status::Fail);
        assert!(live.detail.contains("600s"), "{}", live.detail);

        i.feed_stalled = true;
        let live = evaluate(&i).checks.into_iter().find(|c| c.id == "liveness").unwrap();
        assert_eq!(live.status, Status::Warn);
        assert!(live.detail.contains("Sentinel's own feed"));
    }

    #[test]
    fn warming_up_and_idle_programs_are_not_graded() {
        let mut s = snapshot();
        s.health = "warming_up";
        s.warmup_remaining_secs = 120;
        let h = evaluate(&inputs(&s, &[]));
        assert_eq!(h.status, "learning", "{h:?}");
        assert!(h.score.is_none());

        let mut quiet = snapshot();
        quiet.tx_60s = 3;
        let rel = evaluate(&inputs(&quiet, &[])).checks.into_iter().find(|c| c.id == "reliability").unwrap();
        assert_eq!(rel.status, Status::Unknown);
    }

    #[test]
    fn a_draining_vault_fails_the_funds_check() {
        let s = snapshot();
        let mut i = inputs(&s, &[]);
        i.vaults_watched = 2;
        let funds = evaluate(&i).checks.into_iter().find(|c| c.id == "funds").unwrap();
        assert_eq!(funds.status, Status::Pass);
        assert!(funds.detail.contains("2 vaults"), "{}", funds.detail);

        let mut drain = incident(Severity::High);
        drain.kind = crate::model::IncidentKind::VaultDrain;
        let open = [drain];
        let mut i = inputs(&s, &open);
        i.vaults_watched = 2;
        let h = evaluate(&i);
        let funds = h.checks.iter().find(|c| c.id == "funds").unwrap();
        assert_eq!(funds.status, Status::Fail);
        assert!(h.score.unwrap() < 90);
    }

    #[test]
    fn a_single_key_authority_and_a_missing_idl_cost_points() {
        let s = snapshot();
        let posture = Posture {
            program_id: "P".into(),
            programdata: None,
            upgradeable: true,
            authority: Some("A".into()),
            authority_kind: "single_key",
            last_deployed_slot: None,
            code_bytes: None,
            controller: None,
            risks: vec![],
        };
        let mut i = inputs(&s, &[]);
        let base = evaluate(&i).score.unwrap();
        i.posture = Some(&posture);
        i.idl_loaded = false;
        let worse = evaluate(&i);
        assert!(worse.score.unwrap() < base, "{} !< {base}", worse.score.unwrap());
        assert!(worse.checks.iter().any(|c| c.id == "coverage" && c.status == Status::Warn));
        assert!(worse.checks.iter().any(|c| c.id == "authority" && c.status == Status::Warn));
    }
}
