//! Per-transaction interpretation: failure fingerprints and feed summaries.

use crate::model::{Fingerprint, LargestTransfer, TxSummary};
use crate::pricing::PriceBook;
use vortex::events::{programs, TransferKind, VortexTransaction};

/// Where and why a transaction failed. Uses the deepest failing frame from the
/// log-derived call tree, so a failure inside a CPI is attributed to the
/// program that actually raised it.
pub fn fingerprint(tx: &VortexTransaction) -> Option<Fingerprint> {
    let err = tx.error.as_ref()?;
    let origin = tx
        .invocations
        .iter()
        .filter(|inv| inv.success == Some(false))
        .max_by_key(|inv| inv.depth);

    let program_id = origin
        .map(|inv| inv.program_id.clone())
        .or_else(|| err.program_id.clone())
        .unwrap_or_else(|| "runtime".to_string());
    let instruction = origin.and_then(|inv| inv.instruction.clone());
    let error = err
        .name
        .clone()
        .or_else(|| origin.and_then(|inv| inv.failure.clone()))
        .unwrap_or_else(|| err.message.clone());

    Some(Fingerprint {
        program_id,
        instruction,
        error,
        code: err.custom_code,
    })
}

pub fn symbol_for(mint: Option<&str>) -> String {
    match mint {
        None => "SOL".to_string(),
        Some(m) => programs::known_mint_symbol(m)
            .map(str::to_string)
            .unwrap_or_else(|| short(m)),
    }
}

pub fn short(key: &str) -> String {
    if key.len() <= 10 {
        key.to_string()
    } else {
        format!("{}…{}", &key[..4], &key[key.len() - 4..])
    }
}

pub fn summarize(tx: &VortexTransaction, program_id: &str, prices: &PriceBook) -> TxSummary {
    let mut instructions: Vec<String> = tx
        .instruction_names_for(program_id)
        .map(str::to_string)
        .collect();
    instructions.dedup();

    let moves = tx
        .transfers
        .iter()
        .filter(|t| matches!(t.kind, TransferKind::Sol | TransferKind::Token));
    // Largest by USD when Blur prices anything; otherwise by amount among
    // SOL and well-known tokens (raw amounts of other tokens aren't comparable).
    let priced = moves
        .clone()
        .filter_map(|t| prices.usd(t.mint.as_deref(), t.amount).map(|usd| (t, usd)))
        .max_by(|a, b| a.1.total_cmp(&b.1));
    let largest = match priced {
        Some((t, usd)) => Some(LargestTransfer {
            amount: t.amount,
            symbol: symbol_for(t.mint.as_deref()),
            usd: Some(usd),
        }),
        None => moves
            .filter(|t| t.mint.is_none() || known_stable_or_sol(t.mint.as_deref()))
            .max_by(|a, b| a.amount.total_cmp(&b.amount))
            .map(|t| LargestTransfer {
                amount: t.amount,
                symbol: symbol_for(t.mint.as_deref()),
                usd: None,
            }),
    };

    TxSummary {
        signature: tx.signature.clone(),
        slot: tx.slot,
        received_at: tx.received_at,
        success: tx.success,
        error: tx.error.as_ref().and_then(|e| e.name.clone()),
        fingerprint: fingerprint(tx).map(|f| f.key()),
        compute_units: tx.compute_for(program_id),
        fee: tx.fee,
        fee_payer: tx.fee_payer().map(str::to_string),
        instructions,
        transfers: tx.transfers.len(),
        largest_transfer: largest,
    }
}

fn known_stable_or_sol(mint: Option<&str>) -> bool {
    matches!(
        mint.and_then(programs::known_mint_symbol),
        Some("wSOL" | "USDC" | "USDT" | "PYUSD" | "mSOL" | "JitoSOL")
    )
}
