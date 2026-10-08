//! Alert rules worth having for a program, read from its Anchor IDL. Purely a heuristic over
//! instruction and event names: which ones change who is in charge, stop the program, move
//! money out, or change its parameters. Nothing here fires; it proposes rules a person can
//! apply, and says why each is proposed.

use crate::idl::Idl;
use crate::instruction_rule::name_matches;
use crate::model::Severity;
use serde::Serialize;
use serde_json::{json, Value};

/// What a person must fill in before the rule can be created.
#[derive(Debug, Clone, Serialize)]
pub struct NeedsValue {
    /// The filter this value completes, e.g. `args.amount`.
    pub path: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Suggestion {
    /// Stable for a given IDL, so a client can apply it later.
    pub id: String,
    pub title: String,
    pub why: String,
    pub severity: Severity,
    /// The instruction or event names the rule matches.
    pub matches: Vec<String>,
    /// A `Condition`, as the rules API takes it.
    pub condition: Value,
    pub create_incident: bool,
    pub needs_value: Option<NeedsValue>,
}

const AUTHORITY: &str = "*authority*|*admin*|*owner*|*upgrade*|*governance*";
const PAUSE: &str = "pause*|unpause*|freeze*|thaw*|halt*|emergency*|shutdown*|*kill_switch*";
const FUNDS: &str = "withdraw*|sweep*|drain*|collect_*|*treasury*|*vault_out*|claim_fee*|close*";
const CONFIG: &str = "set_*|update_*|configure*|*config*|*params*|*fee*|*whitelist*|*blacklist*|migrate*";

/// Instructions are assigned to the first group they match, most serious first.
const GROUPS: [(&str, &str, Severity, &str); 4] = [
    ("authority", AUTHORITY, Severity::Critical, "change who controls the program or its settings"),
    ("pause", PAUSE, Severity::High, "stop or restart the program"),
    ("funds", FUNDS, Severity::High, "move funds out or close accounts"),
    ("config", CONFIG, Severity::Medium, "change its parameters"),
];

fn words(group: &str) -> &'static str {
    match group {
        "authority" => "Authority and admin changes",
        "pause" => "Pause and emergency controls",
        "funds" => "Funds leaving",
        _ => "Configuration changes",
    }
}

fn is_amount(name: &str) -> bool {
    let n = name.to_lowercase();
    ["amount", "lamports", "quantity", "shares"].iter().any(|k| n.contains(k))
}

pub fn from_idl(idl: &Idl) -> Vec<Suggestion> {
    let instructions = idl.schema();
    let events = idl.event_schema();
    let mut out = Vec::new();

    let mut left: Vec<&crate::idl::InstructionSchema> = instructions.iter().collect();
    for (group, patterns, severity, what) in GROUPS {
        let (hit, rest): (Vec<_>, Vec<_>) = left.into_iter().partition(|ix| name_matches(patterns, &ix.name));
        left = rest;
        if hit.is_empty() {
            continue;
        }
        let names: Vec<String> = hit.iter().map(|ix| ix.name.clone()).collect();
        out.push(Suggestion {
            id: format!("instruction:{group}"),
            title: format!("{}: {}", words(group), names.iter().take(3).cloned().collect::<Vec<_>>().join(", ") + if names.len() > 3 { ", …" } else { "" }),
            why: format!("{} instruction{} in this program can {what}: {}.", names.len(), if names.len() == 1 { "" } else { "s" }, names.join(", ")),
            severity,
            condition: json!({ "type": "instruction", "name": names.join("|"), "filters": [], "match_mode": "all", "success_only": true, "first_seen_signer": false }),
            matches: names,
            create_incident: true,
            needs_value: None,
        });
        // Money leaving is worth a size threshold too.
        if group == "funds" {
            for ix in hit.iter().filter_map(|ix| ix.args.iter().find(|a| is_amount(&a.path) && matches!(a.r#type.as_str(), "u64" | "u128")).map(|a| (*ix, a))).take(3) {
                let (ix, arg) = ix;
                out.push(Suggestion {
                    id: format!("instruction:large:{}", ix.name),
                    title: format!("Large {}", ix.name),
                    why: format!("`{}` takes a `{}` ({}). Alert when one is above a size you choose; amounts are in the token's smallest unit.", ix.name, arg.path.trim_start_matches("args."), arg.r#type),
                    severity: Severity::High,
                    condition: json!({
                        "type": "instruction", "name": ix.name,
                        "filters": [{ "path": arg.path, "op": "gt", "value": null }],
                        "match_mode": "all", "success_only": true, "first_seen_signer": false
                    }),
                    matches: vec![ix.name.clone()],
                    create_incident: true,
                    needs_value: Some(NeedsValue { path: arg.path.clone(), label: format!("{} above", arg.path.trim_start_matches("args.")) }),
                });
            }
        }
    }

    // Events that announce the same kinds of change.
    let event_groups: [(&str, &str, Severity); 3] = [
        ("authority", AUTHORITY, Severity::High),
        ("pause", PAUSE, Severity::High),
        ("config", CONFIG, Severity::Medium),
    ];
    let mut left: Vec<&crate::idl::EventSchema> = events.iter().collect();
    for (group, patterns, severity) in event_groups {
        let (hit, rest): (Vec<_>, Vec<_>) = left.into_iter().partition(|e| name_matches(patterns, &e.name));
        left = rest;
        if hit.is_empty() {
            continue;
        }
        let names: Vec<String> = hit.iter().map(|e| e.name.clone()).collect();
        out.push(Suggestion {
            id: format!("event:{group}"),
            title: format!("{} events", words(group).trim_end_matches(" changes").trim_end_matches(" controls")),
            why: format!("The program emits {} event{} that report this: {}. Alerting on the event catches it even when the instruction is called through another program.", names.len(), if names.len() == 1 { "" } else { "s" }, names.join(", ")),
            severity,
            condition: json!({ "type": "event", "name": names.join("|"), "filters": [], "match_mode": "all", "success_only": true }),
            matches: names,
            create_incident: true,
            needs_value: None,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Condition;

    fn idl() -> Idl {
        Idl::parse(
            "Prog1111111111111111111111111111111111111",
            &json!({
                "metadata": { "name": "vault", "spec": "0.1.0" },
                "instructions": [
                    { "name": "deposit", "discriminator": [1,0,0,0,0,0,0,0], "accounts": [], "args": [{ "name": "amount", "type": "u64" }] },
                    { "name": "withdraw", "discriminator": [2,0,0,0,0,0,0,0], "accounts": [], "args": [{ "name": "amount", "type": "u64" }] },
                    { "name": "set_authority", "discriminator": [3,0,0,0,0,0,0,0], "accounts": [], "args": [{ "name": "new_authority", "type": "pubkey" }] },
                    { "name": "pause", "discriminator": [4,0,0,0,0,0,0,0], "accounts": [], "args": [] },
                    { "name": "update_fee", "discriminator": [5,0,0,0,0,0,0,0], "accounts": [], "args": [{ "name": "bps", "type": "u16" }] },
                    { "name": "swap", "discriminator": [6,0,0,0,0,0,0,0], "accounts": [], "args": [] }
                ],
                "events": [
                    { "name": "AuthorityChanged", "discriminator": [9,0,0,0,0,0,0,0] },
                    { "name": "Swapped", "discriminator": [10,0,0,0,0,0,0,0] }
                ],
                "types": [
                    { "name": "AuthorityChanged", "type": { "kind": "struct", "fields": [{ "name": "new", "type": "pubkey" }] } },
                    { "name": "Swapped", "type": { "kind": "struct", "fields": [{ "name": "amount", "type": "u64" }] } }
                ]
            }),
        )
        .unwrap()
    }

    #[test]
    fn suggests_rules_for_what_changes_control_stops_or_moves_money() {
        let s = from_idl(&idl());
        let by = |id: &str| s.iter().find(|x| x.id == id).unwrap_or_else(|| panic!("no {id}: {:?}", s.iter().map(|x| &x.id).collect::<Vec<_>>()));
        assert_eq!(by("instruction:authority").matches, vec!["set_authority"]);
        assert_eq!(by("instruction:authority").severity, Severity::Critical);
        assert_eq!(by("instruction:pause").matches, vec!["pause"]);
        assert_eq!(by("instruction:funds").matches, vec!["withdraw"]);
        assert_eq!(by("instruction:config").matches, vec!["update_fee"]);
        let large = by("instruction:large:withdraw");
        assert_eq!(large.needs_value.as_ref().unwrap().path, "args.amount");
        assert!(large.why.contains("smallest unit"));
        assert_eq!(by("event:authority").matches, vec!["AuthorityChanged"]);
        // Ordinary instructions and events are left alone.
        assert!(s.iter().all(|x| !x.matches.iter().any(|m| m == "deposit" || m == "swap" || m == "Swapped")), "{s:?}");
        // Every condition is one the rules API accepts, once a value is filled in.
        for x in &s {
            let mut c = x.condition.clone();
            if x.needs_value.is_some() {
                c["filters"][0]["value"] = json!(1_000_000);
            }
            serde_json::from_value::<Condition>(c).unwrap_or_else(|e| panic!("{}: {e}", x.id));
        }
    }

    #[test]
    fn an_instruction_is_proposed_once_in_its_most_serious_group() {
        let s = from_idl(&idl());
        let mut seen: Vec<&String> = s.iter().filter(|x| x.id.starts_with("instruction:") && !x.id.contains("large")).flat_map(|x| &x.matches).collect();
        let n = seen.len();
        seen.sort();
        seen.dedup();
        assert_eq!(n, seen.len());
    }
}
