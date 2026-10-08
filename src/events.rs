//! Program events: what a program says it did, as opposed to what it was asked to do.
//! Anchor programs emit them two ways. `emit!` writes a `Program data:` log line; `emit_cpi!`
//! calls the program itself with a tagged instruction, which survives log truncation.
//! Both are decoded with the program's IDL.

use crate::idl::Idl;
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use vortex::events::VortexTransaction;

/// First eight bytes of an `emit_cpi!` instruction: the instruction data is this tag, then the event.
pub const EVENT_IX_TAG: [u8; 8] = [0xe4, 0x45, 0xa5, 0x2e, 0x51, 0xcb, 0x9a, 0x1d];

/// A decoded event, as kept for the dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecord {
    pub at: DateTime<Utc>,
    pub signature: String,
    pub name: String,
    pub fields: Value,
}

/// An event a transaction emitted.
#[derive(Debug, Clone)]
pub struct Emitted {
    pub name: String,
    /// `{"fields": .., "signer": .., "event": .., "program": ..}`, for rule filters.
    pub root: Value,
    pub fields: Value,
}

/// A cheap test before decoding: does the transaction carry a log event or a CPI event at all?
pub fn may_carry(tx: &VortexTransaction) -> bool {
    // The base64 of the tag's first eight bytes; every CPI event instruction starts with it.
    const TAG_B64: &str = "5EWlLlHLmh0";
    tx.logs.iter().any(|l| l.starts_with("Program data: ")) || tx.instructions.iter().any(|i| i.data.starts_with(TAG_B64))
}

/// The events `program` logged, as raw payloads, following invoke/success/failed lines. by following invoke/success/failed lines.
fn log_events<'a>(logs: &'a [String], program: &'a str) -> impl Iterator<Item = Vec<u8>> + 'a {
    let mut stack: Vec<&str> = Vec::new();
    logs.iter().filter_map(move |line| {
        let rest = line.strip_prefix("Program ")?;
        if let Some(data) = rest.strip_prefix("data: ") {
            if stack.last() != Some(&program) {
                return None;
            }
            // `sol_log_data` joins several slices with spaces; Anchor writes one.
            let first = data.split_whitespace().next()?;
            return base64::engine::general_purpose::STANDARD.decode(first).ok();
        }
        let (id, what) = rest.split_once(' ')?;
        if what.starts_with("invoke [") {
            stack.push(id);
        } else if what == "success" || what.starts_with("failed") {
            stack.pop();
        }
        None
    })
}

/// Every event `program` emitted in `tx` that `idl` can name: CPI events first, then log events
/// that were not also a CPI event.
pub fn emitted(tx: &VortexTransaction, program: &str, idl: &Idl) -> Vec<Emitted> {
    if !idl.has_events() {
        return Vec::new();
    }
    let signer = tx.fee_payer().unwrap_or_default();
    let mut payloads: Vec<Vec<u8>> = Vec::new();
    let mut paired: Vec<usize> = Vec::new();
    for ix in tx.instructions.iter().filter(|ix| ix.program_id == program) {
        let data = base64::engine::general_purpose::STANDARD.decode(&ix.data).unwrap_or_default();
        if let Some(rest) = data.strip_prefix(&EVENT_IX_TAG) {
            payloads.push(rest.to_vec());
        }
    }
    // Programs such as Pump.fun emit the same event both ways. A log line identical to a CPI
    // event is that event again, not a second one.
    for logged in log_events(&tx.logs, program) {
        match payloads.iter().position(|p| *p == logged) {
            Some(i) if !paired.contains(&i) => paired.push(i),
            _ => payloads.push(logged),
        }
    }
    payloads
        .iter()
        .filter_map(|p| idl.decode_event(p))
        .map(|e| Emitted {
            root: json!({ "fields": e.fields, "signer": signer, "event": e.name, "program": program }),
            name: e.name,
            fields: e.fields,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_programs_own_log_lines_count() {
        let logs: Vec<String> = [
            "Program Outer invoke [1]",
            "Program Mine invoke [2]",
            "Program data: AQID",
            "Program Mine success",
            "Program data: BAUG",
            "Program Outer success",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let got: Vec<_> = log_events(&logs, "Mine").collect();
        assert_eq!(got, vec![vec![1u8, 2, 3]], "the outer program's data line is not Mine's");
    }
}
