//! Alert rules on what an instruction was asked to do: its decoded name,
//! arguments and accounts. Anchor programs are decoded with their on-chain IDL;
//! for other programs the instruction name and parsed fields Vortex provides are used.

use crate::idl::Idl;
use crate::model::{ArgFilter, FilterOp, MatchMode};
use base64::Engine;
use serde_json::{json, Map, Value};
use vortex::events::VortexTransaction;

/// An instruction in a transaction, decoded as far as possible.
#[derive(Debug, Clone)]
pub struct Call {
    /// Position in the transaction ("3" or "3.1" for a CPI).
    pub path: String,
    pub name: String,
    /// `{"args": .., "accounts": {name: pubkey}, "signer": .., "instruction": .., "program": ..}`
    pub root: Value,
    /// Whether an IDL decoded the arguments.
    pub decoded: bool,
}

/// Lowercase with `_`, `-` and spaces removed, so `SetAuthority` equals `set_authority`.
pub fn normalize(name: &str) -> String {
    name.chars().filter(|c| !matches!(c, '_' | '-' | ' ')).flat_map(char::to_lowercase).collect()
}

/// `withdraw`, `set_*|update_*`, `*authority`: patterns separated by `,` or `|`. Empty matches all.
pub fn name_matches(patterns: &str, name: &str) -> bool {
    let patterns: Vec<&str> = patterns.split([',', '|']).map(str::trim).filter(|p| !p.is_empty()).collect();
    if patterns.is_empty() {
        return true;
    }
    let name = normalize(name);
    patterns.iter().any(|p| {
        let p = normalize(p);
        match (p.strip_prefix('*'), p.strip_suffix('*')) {
            (Some(rest), _) if rest.ends_with('*') => name.contains(rest.trim_end_matches('*')),
            (Some(rest), _) => name.ends_with(rest),
            (_, Some(rest)) => name.starts_with(rest),
            _ => name == p,
        }
    })
}

/// The calls `tx` made to `program_id`, decoded with `idl` when there is one.
pub fn calls(tx: &VortexTransaction, program_id: &str, idl: Option<&Idl>) -> Vec<Call> {
    let signer = tx.fee_payer().unwrap_or_default();
    tx.instructions
        .iter()
        .filter(|ix| ix.program_id == program_id)
        .map(|ix| {
            let data = base64::engine::general_purpose::STANDARD.decode(&ix.data).unwrap_or_default();
            if let Some(d) = idl.and_then(|i| i.decode_instruction(&data, &ix.accounts)) {
                let accounts: Map<String, Value> = d.accounts.iter().map(|a| (a.name.clone(), json!(a.pubkey))).collect();
                return Call {
                    path: ix.path.clone(),
                    name: d.name.clone(),
                    root: json!({ "args": d.args, "accounts": accounts, "signer": signer, "instruction": d.name, "program": program_id }),
                    decoded: true,
                };
            }
            let name = ix.name.clone().unwrap_or_default();
            Call {
                path: ix.path.clone(),
                root: json!({
                    "args": ix.parsed.clone().unwrap_or_else(|| json!({})),
                    "accounts": {},
                    "signer": signer,
                    "instruction": name,
                    "program": program_id,
                }),
                name,
                decoded: false,
            }
        })
        .collect()
}

/// The value at `args.amount`, `accounts.authority`, `args.items.0.price`.
pub fn resolve<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').filter(|s| !s.is_empty()).try_fold(root, |v, seg| match v {
        Value::Object(m) => m.get(seg).or_else(|| m.iter().find(|(k, _)| normalize(k) == normalize(seg)).map(|(_, v)| v)),
        Value::Array(a) => seg.parse::<usize>().ok().and_then(|i| a.get(i)),
        _ => None,
    })
}

fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        // The decoder renders u64 values above 2^53 and every u128 as text.
        Value::String(s) => s.trim().replace('_', "").parse().ok(),
        _ => None,
    }
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

pub fn compare(op: FilterOp, actual: &Value, expected: &Value) -> bool {
    match op {
        FilterOp::Eq | FilterOp::Ne => {
            let same = match (number(actual), number(expected)) {
                (Some(a), Some(b)) if !matches!((actual, expected), (Value::String(_), Value::String(_))) => a == b,
                _ => text(actual) == text(expected),
            };
            same == (op == FilterOp::Eq)
        }
        FilterOp::Contains => match actual {
            Value::Array(items) => items.iter().any(|i| text(i) == text(expected)),
            other => text(other).to_lowercase().contains(&text(expected).to_lowercase()),
        },
        _ => match (number(actual), number(expected)) {
            (Some(a), Some(b)) => match op {
                FilterOp::Gt => a > b,
                FilterOp::Gte => a >= b,
                FilterOp::Lt => a < b,
                _ => a <= b,
            },
            _ => false,
        },
    }
}

/// Whether the filters hold, and what the matching ones saw ("amount = 1500000000").
pub fn filters_hold(root: &Value, filters: &[ArgFilter], mode: MatchMode) -> Option<Vec<String>> {
    if filters.is_empty() {
        return Some(Vec::new());
    }
    let seen: Vec<(bool, String)> = filters
        .iter()
        .map(|f| match resolve(root, &f.path) {
            Some(actual) => (compare(f.op, actual, &f.value), format!("{} = {}", f.path.trim_start_matches("args."), text(actual))),
            None => (false, format!("{} missing", f.path)),
        })
        .collect();
    let holds = match mode {
        MatchMode::All => seen.iter().all(|(ok, _)| *ok),
        MatchMode::Any => seen.iter().any(|(ok, _)| *ok),
    };
    holds.then(|| seen.into_iter().filter(|(ok, _)| *ok).map(|(_, s)| s).collect())
}

/// A number worth recording as the alert's value: the first numeric filtered field.
pub fn headline_value(root: &Value, filters: &[ArgFilter]) -> f64 {
    filters.iter().find_map(|f| resolve(root, &f.path).and_then(number)).unwrap_or(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ArgFilter;

    fn f(path: &str, op: FilterOp, value: Value) -> ArgFilter {
        ArgFilter { path: path.into(), op, value }
    }

    #[test]
    fn names_match_across_styles_and_patterns() {
        assert!(name_matches("", "anything"));
        assert!(name_matches("SetAuthority", "set_authority"));
        assert!(name_matches("withdraw", "Withdraw"));
        assert!(!name_matches("withdraw", "withdraw_fees"));
        assert!(name_matches("set_*|update_*", "update_config"));
        assert!(name_matches("*authority", "set_authority"));
        assert!(name_matches("*fee*, pause", "collect_fees"));
        assert!(name_matches("pause", "pause"));
        assert!(!name_matches("set_*", "reset_config"), "a prefix pattern is anchored at the start");
    }

    #[test]
    fn numbers_compare_even_when_the_decoder_renders_them_as_text() {
        let root = json!({ "args": { "amount": "18446744073709551615", "small": 5, "kind": "Swap", "list": ["a", "b"] }, "signer": "Wallet1" });
        assert!(compare(FilterOp::Gt, resolve(&root, "args.amount").unwrap(), &json!(1_000_000_000u64)));
        assert!(compare(FilterOp::Lte, resolve(&root, "args.small").unwrap(), &json!("5")));
        assert!(!compare(FilterOp::Gt, resolve(&root, "args.kind").unwrap(), &json!(1)), "text isn't a number");
        assert!(compare(FilterOp::Eq, resolve(&root, "signer").unwrap(), &json!("Wallet1")));
        assert!(compare(FilterOp::Ne, resolve(&root, "signer").unwrap(), &json!("Other")));
        assert!(compare(FilterOp::Contains, resolve(&root, "args.list").unwrap(), &json!("b")));
        assert!(compare(FilterOp::Contains, resolve(&root, "args.kind").unwrap(), &json!("wap")));
        assert!(resolve(&root, "args.nope").is_none());
    }

    #[test]
    fn all_and_any_and_what_was_seen() {
        let root = json!({ "args": { "amount": 2_000_000_000u64, "slippage": 5 }, "accounts": { "authority": "Auth1" } });
        let big = f("args.amount", FilterOp::Gt, json!(1_000_000_000u64));
        let low = f("args.slippage", FilterOp::Lt, json!(1));
        let seen = filters_hold(&root, &[big.clone()], MatchMode::All).unwrap();
        assert_eq!(seen, vec!["amount = 2000000000"]);
        assert!(filters_hold(&root, &[big.clone(), low.clone()], MatchMode::All).is_none());
        assert_eq!(filters_hold(&root, &[big, low], MatchMode::Any).unwrap().len(), 1);
        assert!(filters_hold(&root, &[f("accounts.authority", FilterOp::Eq, json!("Auth1"))], MatchMode::All).is_some());
        assert!(filters_hold(&root, &[f("args.missing", FilterOp::Gt, json!(0))], MatchMode::All).is_none(), "a field that isn't there never matches");
        assert_eq!(filters_hold(&root, &[], MatchMode::All), Some(vec![]));
    }

    #[test]
    fn field_names_are_found_whatever_their_style() {
        let root = json!({ "args": { "max_sol_cost": 7 } });
        assert_eq!(resolve(&root, "args.maxSolCost"), Some(&json!(7)));
        assert_eq!(headline_value(&root, &[f("args.max_sol_cost", FilterOp::Gt, json!(1))]), 7.0);
    }
}
