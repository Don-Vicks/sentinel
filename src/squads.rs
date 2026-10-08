//! Squads multisigs, the most common way to hold a program's upgrade authority.
//! Sentinel doesn't follow proposals; it recognises when a Squads instruction
//! executed an upgrade (through CPI) and reads the multisig's own settings, so
//! an alert can say who signed and how many signatures a change needs.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use vortex::events::{Instruction, VortexTransaction};

pub const V4: &str = "SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf";
pub const V3: &str = "SMPLecH534NA9acpos4G6x7uf3LWbCAwZQE9e8ZekMu";

pub fn name_of(program_id: &str) -> Option<&'static str> {
    match program_id {
        V4 => Some("Squads v4"),
        V3 => Some("Squads v3"),
        _ => None,
    }
}

/// Anchor instructions (by name) of each Squads version that matter when reading a transaction.
fn instruction_names(program_id: &str) -> &'static [&'static str] {
    match program_id {
        V4 => &[
            "vault_transaction_execute",
            "config_transaction_execute",
            "batch_execute_transaction",
            "vault_transaction_create",
            "config_transaction_create",
            "proposal_create",
            "proposal_activate",
            "proposal_approve",
            "proposal_reject",
            "proposal_cancel",
        ],
        V3 => &["execute_transaction", "execute_instruction", "create_transaction", "activate_transaction", "approve", "reject"],
        _ => &[],
    }
}

/// The Anchor instruction a Squads call was, from its 8-byte discriminator.
pub fn instruction_name(program_id: &str, data: &[u8]) -> Option<&'static str> {
    let disc = data.get(..8)?;
    instruction_names(program_id)
        .iter()
        .copied()
        .find(|name| solana_sdk::hash::hashv(&[b"global:", name.as_bytes()]).to_bytes()[..8] == *disc)
}

/// A Squads call that ran another instruction.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Via {
    pub program: &'static str,
    pub program_id: String,
    pub multisig: String,
    /// The member that signed the execution.
    pub executor: Option<String>,
    pub instruction: Option<&'static str>,
}

/// If the instruction at `path` ("3.1") was run by a Squads call at the top level ("3"), which one.
pub fn executed_by(tx: &VortexTransaction, path: &str) -> Option<Via> {
    use base64::Engine;
    let (top, _) = path.split_once('.')?;
    let parent: &Instruction = tx.instructions.iter().find(|ix| ix.path == top)?;
    let program = name_of(&parent.program_id)?;
    let data = base64::engine::general_purpose::STANDARD.decode(&parent.data).unwrap_or_default();
    let multisig = parent.accounts.first()?.clone();
    let executor = parent
        .accounts
        .iter()
        .filter(|a| **a != multisig)
        .find(|a| tx.accounts.iter().any(|t| t.signer && &t.pubkey == *a))
        .cloned();
    Some(Via {
        program,
        program_id: parent.program_id.clone(),
        instruction: instruction_name(&parent.program_id, &data),
        multisig,
        executor,
    })
}

/// What a multisig requires to act.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MultisigInfo {
    /// Approvals needed.
    pub threshold: u16,
    pub members: usize,
    /// Seconds a passed proposal waits before it can execute.
    pub time_lock: u32,
}

impl MultisigInfo {
    pub fn describe(&self) -> String {
        let lock = if self.time_lock > 0 { format!(", {} time lock", span(self.time_lock as i64)) } else { String::new() };
        format!("{} of {} signatures{lock}", self.threshold, self.members)
    }
}

fn span(secs: i64) -> String {
    match secs {
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 172_800 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

/// Reads a Squads v4 `Multisig` account: discriminator, create_key, config_authority,
/// threshold u16, time_lock u32, transaction_index u64, stale_transaction_index u64,
/// rent_collector Option<Pubkey>, bump u8, members Vec<(Pubkey, permissions u8)>.
/// Returns `None` unless the layout is internally consistent, so a mistaken read is never shown.
pub fn parse_multisig(data: &[u8]) -> Option<MultisigInfo> {
    let mut at = 8 + 32 + 32;
    let threshold = u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?);
    at += 2;
    let time_lock = u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?);
    at += 4 + 8 + 8;
    let collector = *data.get(at)?;
    at += 1 + if collector == 1 { 32 } else { 0 };
    if collector > 1 {
        return None;
    }
    at += 1; // bump
    let len = u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?) as usize;
    at += 4;
    let end = at.checked_add(len.checked_mul(33)?)?;
    if data.len() < end || len == 0 || len > 65_535 || threshold == 0 || threshold as usize > len {
        return None;
    }
    // Space reserved for more members is zeroed, never data.
    if data[end..].iter().any(|b| *b != 0) {
        return None;
    }
    Some(MultisigInfo { threshold, members: len, time_lock })
}

pub async fn fetch_multisig(rpc: &RpcClient, address: &str) -> Result<Option<MultisigInfo>> {
    let account = rpc.get_account(&Pubkey::from_str(address)?).await?;
    if account.owner.to_string() != V4 {
        return Ok(None);
    }
    Ok(parse_multisig(&account.data))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn multisig_bytes(threshold: u16, members: usize, time_lock: u32, collector: bool) -> Vec<u8> {
        let mut d = vec![0u8; 8 + 64];
        d.extend(threshold.to_le_bytes());
        d.extend(time_lock.to_le_bytes());
        d.extend(7u64.to_le_bytes());
        d.extend(3u64.to_le_bytes());
        if collector {
            d.push(1);
            d.extend([9u8; 32]);
        } else {
            d.push(0);
        }
        d.push(254);
        d.extend((members as u32).to_le_bytes());
        for i in 0..members {
            d.extend([i as u8 + 1; 32]);
            d.push(7);
        }
        d
    }

    #[test]
    fn reads_a_consistent_multisig() {
        let info = parse_multisig(&multisig_bytes(3, 5, 0, false)).unwrap();
        assert_eq!(info, MultisigInfo { threshold: 3, members: 5, time_lock: 0 });
        assert_eq!(info.describe(), "3 of 5 signatures");
        let with_collector = parse_multisig(&multisig_bytes(2, 4, 86_400, true)).unwrap();
        assert_eq!((with_collector.threshold, with_collector.members), (2, 4));
        assert!(with_collector.describe().contains("2 of 4") && with_collector.describe().contains("time lock"));
        // Room left for more members is fine.
        let mut padded = multisig_bytes(2, 3, 0, false);
        padded.extend([0u8; 99]);
        assert!(parse_multisig(&padded).is_some());
    }

    #[test]
    fn refuses_anything_that_does_not_add_up() {
        assert!(parse_multisig(&multisig_bytes(6, 5, 0, false)).is_none(), "threshold above the member count");
        assert!(parse_multisig(&multisig_bytes(0, 5, 0, false)).is_none(), "zero threshold");
        assert!(parse_multisig(&multisig_bytes(1, 0, 0, false)).is_none(), "no members");
        let mut trailing = multisig_bytes(2, 3, 0, false);
        trailing.push(1);
        assert!(parse_multisig(&trailing).is_none(), "trailing data that isn't padding");
        let truncated = multisig_bytes(2, 3, 0, false);
        assert!(parse_multisig(&truncated[..truncated.len() - 10]).is_none());
        assert!(parse_multisig(&[0u8; 12]).is_none());
    }

    #[test]
    fn instructions_are_named_by_their_anchor_discriminator() {
        let disc = |name: &str| solana_sdk::hash::hashv(&[b"global:", name.as_bytes()]).to_bytes()[..8].to_vec();
        assert_eq!(instruction_name(V4, &disc("vault_transaction_execute")), Some("vault_transaction_execute"));
        assert_eq!(instruction_name(V4, &disc("config_transaction_execute")), Some("config_transaction_execute"));
        assert_eq!(instruction_name(V3, &disc("execute_transaction")), Some("execute_transaction"));
        assert_eq!(instruction_name(V3, &disc("vault_transaction_execute")), None, "wrong version");
        assert_eq!(instruction_name(V4, &[1, 2, 3]), None);
    }
}
