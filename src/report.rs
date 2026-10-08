//! Turns an incident into something a person (or an agent) can act on: a
//! structured diagnosis and a markdown post-mortem. Everything here is derived
//! from what Sentinel recorded; nothing is guessed, and each claim carries the
//! evidence it rests on.

use crate::model::{Incident, IncidentKind, IncidentStatus, Severity, TxSummary};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Similar {
    pub id: i64,
    pub title: String,
    pub detected_at: i64,
    pub resolved: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Diagnosis {
    pub incident_id: i64,
    pub kind: IncidentKind,
    pub severity: Severity,
    /// One paragraph: what this is.
    pub verdict: String,
    /// The most likely cause, when the evidence points at one.
    pub likely_cause: Option<String>,
    /// "high" when several independent signals agree, "low" when this is a guess from one.
    pub confidence: &'static str,
    pub evidence: Vec<String>,
    pub similar_incidents: Vec<Similar>,
    pub suggested_steps: Vec<String>,
}

fn fingerprint(inc: &Incident) -> Option<&serde_json::Value> {
    inc.evidence["fingerprints"].get(0)
}

/// Stable identity of the top failure, for finding repeats.
fn top_key(inc: &Incident) -> Option<String> {
    fingerprint(inc).and_then(|f| f["key"].as_str()).map(String::from)
}

fn describe_top(inc: &Incident) -> Option<String> {
    let f = fingerprint(inc)?;
    let error = f["error"].as_str()?;
    let program = f["program_name"].as_str().unwrap_or("a program");
    let ix = f["instruction"].as_str().map(|i| format!("::{i}")).unwrap_or_default();
    Some(format!("{program}{ix} → {error}"))
}

/// Earlier incidents like this one: same program and kind, and the same top failure when it has one.
pub fn similar(inc: &Incident, history: &[Incident]) -> Vec<Similar> {
    let key = top_key(inc);
    history
        .iter()
        .filter(|h| h.id != inc.id && h.program_id == inc.program_id && h.kind == inc.kind)
        .filter(|h| key.is_none() || top_key(h) == key)
        .take(5)
        .map(|h| Similar {
            id: h.id,
            title: h.title.clone(),
            detected_at: h.detected_at.timestamp(),
            resolved: h.status == IncidentStatus::Resolved,
        })
        .collect()
}

pub fn diagnose(inc: &Incident, history: &[Incident]) -> Diagnosis {
    let mut evidence = Vec::new();
    let mut steps = Vec::new();
    let mut likely_cause = None;
    let mut signals = 0u32;

    if let Some(top) = describe_top(inc) {
        let share = fingerprint(inc).and_then(|f| f["share"].as_f64()).unwrap_or(0.0);
        evidence.push(format!("Top failure: {top} ({:.0}% of the failed transactions linked to this incident).", share * 100.0));
        if share >= 0.6 {
            signals += 1;
        }
        // An error raised by another program is that program's, or something it calls.
        if let Some(raised) = fingerprint(inc).and_then(|f| f["program_id"].as_str()) {
            if raised != inc.program_id {
                evidence.push(format!(
                    "The error is raised by {}, not by the monitored program: the failure is in something it calls or is called through.",
                    fingerprint(inc).and_then(|f| f["program_name"].as_str()).unwrap_or(raised)
                ));
                likely_cause = Some(format!("{top}, outside the monitored program"));
            }
        }
    }

    if let Some(deploy) = inc.evidence.get("deploy") {
        signals += 2;
        if let Some(note) = deploy["note"].as_str() {
            evidence.push(note.to_string());
        }
        likely_cause = Some(match describe_top(inc) {
            Some(top) => format!("A regression from the recent program upgrade: {top} began after it."),
            None => "The recent program upgrade.".to_string(),
        });
        steps.push("Compare the upgrade's transaction and buffer with what you intended to ship; consider rolling back if the failure is user-facing.".to_string());
    }

    match inc.kind {
        IncidentKind::AuthorityChange => {
            signals += 2;
            let a = &inc.evidence["authority"];
            let action = a["action"].as_str().unwrap_or("change");
            likely_cause = Some(format!("The program's {action} was executed on chain by {}.", a["authority"].as_str().unwrap_or("an unknown signer")));
            evidence.push(inc.summary.clone());
            if a["path"].as_str().is_some_and(|p| p.contains('.')) {
                evidence.push("It ran via CPI, which is how a multisig or governance program executes an approved change.".into());
            } else {
                evidence.push("It was signed directly, not via a multisig or governance program.".into());
                steps.push("A direct upgrade means one key can change the program alone; consider moving the authority to a multisig.".into());
            }
            steps.push("Confirm this matches your release process: who signed, and was it approved?".into());
            steps.push("If it was not you, treat it as a security incident: rotate keys, pause integrations, and review the new authority.".into());
        }
        IncidentKind::VaultDrain => {
            signals += 1;
            let v = &inc.evidence["vault"];
            likely_cause = Some(format!(
                "Funds are leaving vault {} ({:.0}% of its balance).",
                v["account"].as_str().unwrap_or("?"),
                v["pct"].as_f64().unwrap_or(0.0)
            ));
            evidence.push(inc.summary.clone());
            steps.push("Open the linked transactions: is every outflow a user withdrawal with a matching burn or position change?".into());
            steps.push("Check who signed and which instruction moved the funds; an admin or upgrade-authority signer is the red flag.".into());
            steps.push("If it is not legitimate, pause the program (if it has a pause) and alert the team now.".into());
        }
        IncidentKind::FailureSpike | IncidentKind::ErrorSpike | IncidentKind::RuleTriggered => {
            if inc.affected_wallets > 0 {
                let per = inc.affected_count as f64 / inc.affected_wallets as f64;
                evidence.push(format!(
                    "{} failed transactions from {} wallets ({per:.1} each).",
                    inc.affected_count, inc.affected_wallets
                ));
                if per >= 5.0 && inc.affected_wallets <= 3 {
                    evidence.push("A few wallets account for almost all of it, which looks like a retrying bot rather than users.".into());
                    steps.push("Check whether the few wallets involved are your own keeper or an external bot; a retry loop may need rate limiting.".into());
                    likely_cause.get_or_insert_with(|| "A few wallets retrying the same failing call.".to_string());
                }
            }
            if likely_cause.is_none() {
                likely_cause = describe_top(inc).map(|t| format!("{t} is the dominant failure."));
            }
            steps.push("Open the example transactions for the top failure and read the program logs around the error.".into());
            if inc.evidence.get("deploy").is_none() {
                steps.push("No upgrade preceded this. Look for a changed dependency (oracle, AMM, token program), a market move, or a frontend release.".into());
            }
        }
        IncidentKind::ActivityDrop => {
            likely_cause = Some("Transactions stopped arriving while the chain kept moving.".into());
            steps.push("Check whether your frontend, RPC provider or keeper is up; traffic does not usually stop on its own.".into());
            steps.push("Check Sentinel's own feed status before assuming the program is down.".into());
        }
        IncidentKind::ActivitySpike => {
            evidence.push(format!("{} transactions from {} wallets.", inc.affected_count, inc.affected_wallets));
            steps.push("Check wallet concentration in the linked transactions: a handful of wallets is a bot or an attack, many is real demand.".into());
        }
        IncidentKind::ComputeSpike => {
            steps.push("Compare compute use per instruction with normal; a new code path or larger accounts raises it.".into());
        }
        IncidentKind::LargeTransfer => {
            evidence.push(inc.summary.clone());
            steps.push("Confirm the transfer is expected for this program (a known treasury move or a large user).".into());
        }
    }

    let similar = similar(inc, history);
    if let Some(prev) = similar.first() {
        evidence.push(format!(
            "{} earlier incident{} of this kind{} (latest #{}).",
            similar.len(),
            if similar.len() == 1 { "" } else { "s" },
            if top_key(inc).is_some() { " with the same top failure" } else { "" },
            prev.id
        ));
        signals += 1;
        steps.push(format!("See how #{} was handled; the same cause may be back.", prev.id));
    }

    let confidence = match signals {
        0 | 1 => "low",
        2 => "medium",
        _ => "high",
    };
    let state = match inc.status {
        IncidentStatus::Resolved => "resolved",
        IncidentStatus::Investigating => "being investigated",
        IncidentStatus::Open => "open",
    };
    let verdict = format!("{} ({:?} severity, {state}). {}", inc.title, inc.severity, inc.summary);
    Diagnosis {
        incident_id: inc.id,
        kind: inc.kind,
        severity: inc.severity,
        verdict,
        likely_cause,
        confidence,
        evidence,
        similar_incidents: similar,
        suggested_steps: steps,
    }
}

fn when(t: chrono::DateTime<chrono::Utc>) -> String {
    t.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}

fn span(secs: i64) -> String {
    match secs {
        s if s < 90 => format!("{s}s"),
        s if s < 5400 => format!("{}m", s / 60),
        s => format!("{:.1}h", s as f64 / 3600.0),
    }
}

/// A post-mortem in markdown: what happened, when, why it likely happened and what to do.
pub fn markdown(inc: &Incident, label: &str, txs: &[TxSummary], history: &[Incident], link: &str) -> String {
    let d = diagnose(inc, history);
    let mut out = String::new();
    out.push_str(&format!("# {} (#{})\n\n", inc.title, inc.id));
    out.push_str(&format!(
        "**Program:** {label} · **Severity:** {:?} · **Status:** {:?}\n\n",
        inc.severity, inc.status
    ));
    if !link.is_empty() {
        out.push_str(&format!("[Open in Sentinel]({link})\n\n"));
    }

    out.push_str("## Summary\n\n");
    out.push_str(&format!("{}\n\n{}\n\n", inc.summary, inc.explanation));

    out.push_str("## Timeline\n\n");
    if let Some(onset) = inc.onset_at {
        out.push_str(&format!("- **{}** onset\n", when(onset)));
    }
    out.push_str(&format!(
        "- **{}** detected{}\n",
        when(inc.detected_at),
        inc.detection_latency_ms.map(|ms| format!(" ({:.1}s after the first affected transaction)", ms as f64 / 1000.0)).unwrap_or_default()
    ));
    match inc.resolved_at {
        Some(r) => out.push_str(&format!(
            "- **{}** resolved (lasted {} from detection)\n",
            when(r),
            span((r - inc.detected_at).num_seconds().max(0))
        )),
        None => out.push_str("- Still open\n"),
    }
    out.push('\n');

    if let Some(deploy) = inc.evidence.get("deploy").and_then(|d| d["note"].as_str()) {
        out.push_str(&format!("## Recent upgrade\n\n{deploy}\n\n"));
    }
    if let Some(a) = inc.evidence.get("authority") {
        out.push_str("## What changed on chain\n\n");
        out.push_str(&format!(
            "- Action: `{}`\n- Signed by: `{}`\n- Transaction: `{}` (slot {})\n\n",
            a["action"].as_str().unwrap_or("?"),
            a["authority"].as_str().unwrap_or("unknown"),
            a["signature"].as_str().unwrap_or("?"),
            a["slot"]
        ));
    }
    if let Some(v) = inc.evidence.get("vault") {
        out.push_str("## Vault\n\n");
        out.push_str(&format!(
            "- Account: `{}`\n- Net outflow: {:.4} {} ({:.0}% of its balance){}\n- Left: {:.4} {}\n\n",
            v["account"].as_str().unwrap_or("?"),
            v["outflow"].as_f64().unwrap_or(0.0),
            v["symbol"].as_str().unwrap_or(""),
            v["pct"].as_f64().unwrap_or(0.0),
            v["usd"].as_f64().map(|u| format!(", about ${u:.0}")).unwrap_or_default(),
            v["balance_after"].as_f64().unwrap_or(0.0),
            v["symbol"].as_str().unwrap_or("")
        ));
    }

    if let Some(fps) = inc.evidence["fingerprints"].as_array().filter(|f| !f.is_empty()) {
        out.push_str("## Failures\n\n| Error | Raised by | Share | Count |\n|---|---|---|---|\n");
        for f in fps.iter().take(8) {
            out.push_str(&format!(
                "| {} | {}{} | {:.0}% | {} |\n",
                f["error"].as_str().unwrap_or("?"),
                f["program_name"].as_str().unwrap_or("?"),
                f["instruction"].as_str().map(|i| format!("::{i}")).unwrap_or_default(),
                f["share"].as_f64().unwrap_or(0.0) * 100.0,
                f["count"]
            ));
        }
        out.push('\n');
    }

    out.push_str(&format!(
        "## Impact\n\n- {} affected transactions from {} wallets\n\n",
        inc.affected_count, inc.affected_wallets
    ));

    out.push_str("## Diagnosis\n\n");
    out.push_str(&format!("Confidence: **{}**\n\n", d.confidence));
    if let Some(cause) = &d.likely_cause {
        out.push_str(&format!("**Likely cause:** {cause}\n\n"));
    }
    for e in &d.evidence {
        out.push_str(&format!("- {e}\n"));
    }
    out.push('\n');

    if !d.suggested_steps.is_empty() {
        out.push_str("## Next steps\n\n");
        for (i, step) in d.suggested_steps.iter().enumerate() {
            out.push_str(&format!("{}. {step}\n", i + 1));
        }
        out.push('\n');
    }

    if !txs.is_empty() {
        out.push_str("## Example transactions\n\n");
        for t in txs.iter().take(10) {
            out.push_str(&format!(
                "- [`{}`](https://solscan.io/tx/{}){}\n",
                &t.signature[..t.signature.len().min(12)],
                t.signature,
                t.error.as_deref().map(|e| format!(": {e}")).unwrap_or_default()
            ));
        }
        out.push('\n');
    }
    out.push_str("---\n_Generated by Vortex Sentinel from recorded data._\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn incident(id: i64, kind: &str, evidence: serde_json::Value) -> Incident {
        serde_json::from_value(json!({
            "id": id, "program_id": "P", "kind": kind, "severity": "high", "status": "open",
            "title": "Transaction failure spike · Pump.fun", "summary": "45% of transactions failing",
            "explanation": "Failure rate rose.", "source": "detector",
            "metric": null, "observed": null, "peak": null, "baseline": null, "threshold": null,
            "onset_at": "2024-01-01T00:00:00Z", "detected_at": "2024-01-01T00:00:05Z",
            "updated_at": "2024-01-01T00:00:05Z", "resolved_at": null, "detection_latency_ms": 4200,
            "affected_count": 90, "affected_wallets": 30, "evidence": evidence
        }))
        .unwrap()
    }

    fn fingerprint_evidence(program_id: &str) -> serde_json::Value {
        json!({ "fingerprints": [{ "key": "P:Sell:TooLittle", "program_id": program_id, "program_name": "Pump.fun",
                 "instruction": "Sell", "error": "TooLittleSolReceived", "code": 6003, "count": 80, "share": 0.9, "samples": [] }] })
    }

    #[test]
    fn a_failure_after_an_upgrade_blames_the_upgrade() {
        let mut ev = fingerprint_evidence("P");
        ev["deploy"] = json!({ "note": "This began 74s after the program was upgraded (transaction abcd…, incident #1002)." });
        let d = diagnose(&incident(1010, "failure_spike", ev), &[]);
        assert_eq!(d.confidence, "high", "{d:?}");
        assert!(d.likely_cause.as_deref().unwrap().contains("upgrade"), "{:?}", d.likely_cause);
        assert!(d.evidence.iter().any(|e| e.contains("74s after")));
        assert!(d.suggested_steps.iter().any(|s| s.contains("roll")));
    }

    #[test]
    fn an_error_from_another_program_is_not_blamed_on_this_one() {
        let d = diagnose(&incident(2, "failure_spike", fingerprint_evidence("SomeOtherProgram")), &[]);
        assert!(d.likely_cause.unwrap().contains("outside the monitored program"));
        assert!(d.evidence.iter().any(|e| e.contains("not by the monitored program")));
    }

    #[test]
    fn repeats_are_found_and_unrelated_incidents_are_not() {
        let now = incident(10, "failure_spike", fingerprint_evidence("P"));
        let earlier = incident(7, "failure_spike", fingerprint_evidence("P"));
        let other_error = {
            let mut e = fingerprint_evidence("P");
            e["fingerprints"][0]["key"] = json!("P:Buy:Slippage");
            incident(8, "failure_spike", e)
        };
        let other_kind = incident(9, "compute_spike", json!({}));
        let found = similar(&now, &[now.clone(), earlier, other_error, other_kind]);
        assert_eq!(found.iter().map(|s| s.id).collect::<Vec<_>>(), vec![7]);
    }

    #[test]
    fn an_unexpected_direct_upgrade_is_flagged() {
        let inc = incident(
            3,
            "authority_change",
            json!({ "authority": { "action": "upgrade", "authority": "Auth1", "path": "0", "signature": "sig", "slot": 5 } }),
        );
        let d = diagnose(&inc, &[]);
        assert!(d.evidence.iter().any(|e| e.contains("signed directly")));
        assert!(d.suggested_steps.iter().any(|s| s.contains("multisig")));
        assert!(d.suggested_steps.iter().any(|s| s.contains("security incident")));
    }

    #[test]
    fn the_post_mortem_has_the_sections_a_reader_needs() {
        let mut ev = fingerprint_evidence("P");
        ev["deploy"] = json!({ "note": "This began 74s after the program was upgraded." });
        let mut inc = incident(11, "failure_spike", ev);
        inc.status = IncidentStatus::Resolved;
        inc.resolved_at = Some(inc.detected_at + chrono::Duration::minutes(12));
        let md = markdown(&inc, "Pump.fun", &[], &[], "https://sentinel.example/incidents/11");
        for section in ["# Transaction failure spike", "## Timeline", "resolved (lasted 12m", "## Recent upgrade", "| TooLittleSolReceived |", "## Diagnosis", "## Next steps", "[Open in Sentinel]"] {
            assert!(md.contains(section), "missing {section:?} in:\n{md}");
        }
    }
}
