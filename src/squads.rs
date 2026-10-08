//! Squads multisigs (v3, v4 and v5, the Smart Account program), the most common way to hold a
//! program's upgrade authority. Sentinel recognises when a Squads instruction executed an
//! upgrade (through CPI), reads the multisig's own settings so an alert can say who signed and
//! how many signatures a change needs, and can alert on any Squads action on a multisig.
//! It does not follow a proposal from creation to execution.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use vortex::events::{Instruction, VortexTransaction};

pub const V4: &str = "SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf";
/// Squads v5, the Smart Account program. Its multisig is called a `Settings` account.
pub const V5: &str = "SMRTzfY6DfH5ik3TKiyLFfXexV8uSG3d2UksSCYdunG";
pub const V3: &str = "SMPLecH534NA9acpos4G6x7uf3LWbCAwZQE9e8ZekMu";

pub fn name_of(program_id: &str) -> Option<&'static str> {
    match program_id {
        V5 => Some("Squads v5"),
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
        V5 => &[
            "execute_transaction",
            "execute_settings_transaction",
            "execute_batch_transaction",
            "execute_transaction_sync",
            "execute_settings_transaction_sync",
            "create_transaction",
            "create_settings_transaction",
            "create_batch",
            "create_proposal",
            "activate_proposal",
            "approve_proposal",
            "reject_proposal",
            "cancel_proposal",
            "use_spending_limit",
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

/// Reads a Squads v5 `Settings` account: discriminator, seed u128, settings_authority,
/// threshold u16, time_lock u32, transaction_index u64, stale_transaction_index u64,
/// archival_authority Option<Pubkey>, archivable_after u64, bump u8,
/// signers Vec<(Pubkey, permissions u8)>, then three bytes. Same rule as `parse_multisig`:
/// `None` unless the layout is internally consistent.
pub fn parse_settings(data: &[u8]) -> Option<MultisigInfo> {
    let mut at = 8 + 16 + 32;
    let threshold = u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?);
    at += 2;
    let time_lock = u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?);
    at += 4 + 8 + 8;
    let archival = *data.get(at)?;
    if archival > 1 {
        return None;
    }
    at += 1 + if archival == 1 { 32 } else { 0 };
    at += 8 + 1; // archivable_after, bump
    let len = u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?) as usize;
    at += 4;
    let end = at.checked_add(len.checked_mul(33)?)?;
    if data.len() < end || len == 0 || len > 65_535 || threshold == 0 || threshold as usize > len {
        return None;
    }
    // After the signers: account_utilization and two reserved bytes, then zeroed space.
    if data.len() < end + 3 || data[end + 3..].iter().any(|b| *b != 0) {
        return None;
    }
    Some(MultisigInfo { threshold, members: len, time_lock })
}

pub async fn fetch_multisig(rpc: &RpcClient, address: &str) -> Result<Option<MultisigInfo>> {
    let account = rpc.get_account(&Pubkey::from_str(address)?).await?;
    Ok(match account.owner.to_string().as_str() {
        V4 => parse_multisig(&account.data),
        V5 => parse_settings(&account.data),
        _ => None,
    })
}

/// The address of vault number `index` of a multisig (v4) or smart account (v5).
pub fn vault_address(program_id: &str, multisig: &str, index: u8) -> Option<String> {
    let multisig = Pubkey::from_str(multisig).ok()?;
    let seeds: [&[u8]; 4] = match program_id {
        V4 => [b"multisig", multisig.as_ref(), b"vault", &[index]],
        V5 => [b"smart_account", multisig.as_ref(), b"smart_account", &[index]],
        _ => return None,
    };
    let program = Pubkey::from_str(program_id).ok()?;
    Some(Pubkey::find_program_address(&seeds, &program).0.to_string())
}

/// A call to a Squads program about one multisig.
#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    pub program: &'static str,
    pub program_id: String,
    /// `None` when the discriminator isn't one Sentinel knows.
    pub instruction: Option<&'static str>,
    /// The first account of the instruction that signed the transaction, usually the member.
    pub member: Option<String>,
}

/// The Squads calls in `tx` made on `multisig` (the first account of the call).
pub fn actions(tx: &VortexTransaction, multisig: &str) -> Vec<Action> {
    use base64::Engine;
    tx.instructions
        .iter()
        .filter(|ix| ix.inner_index.is_none() && name_of(&ix.program_id).is_some() && ix.accounts.first().is_some_and(|a| a == multisig))
        .map(|ix| {
            let data = base64::engine::general_purpose::STANDARD.decode(&ix.data).unwrap_or_default();
            Action {
                program: name_of(&ix.program_id).unwrap_or("Squads"),
                program_id: ix.program_id.clone(),
                instruction: instruction_name(&ix.program_id, &data),
                member: ix.accounts.iter().skip(1).find(|a| tx.accounts.iter().any(|t| t.signer && &t.pubkey == *a)).cloned(),
            }
        })
        .collect()
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

    fn settings_bytes(threshold: u16, signers: usize, time_lock: u32, archival: bool) -> Vec<u8> {
        let mut d = vec![0u8; 8 + 16 + 32];
        d.extend(threshold.to_le_bytes());
        d.extend(time_lock.to_le_bytes());
        d.extend(7u64.to_le_bytes());
        d.extend(3u64.to_le_bytes());
        if archival {
            d.push(1);
            d.extend([9u8; 32]);
        } else {
            d.push(0);
        }
        d.extend(0u64.to_le_bytes());
        d.push(255);
        d.extend((signers as u32).to_le_bytes());
        for i in 0..signers {
            d.extend([i as u8 + 1; 32]);
            d.push(7);
        }
        d.extend([1u8, 0, 0]);
        d
    }

    #[test]
    fn reads_a_v5_settings_account_and_refuses_what_does_not_add_up() {
        let info = parse_settings(&settings_bytes(2, 3, 3600, false)).unwrap();
        assert_eq!(info, MultisigInfo { threshold: 2, members: 3, time_lock: 3600 });
        assert_eq!(parse_settings(&settings_bytes(4, 7, 0, true)).unwrap().members, 7, "an archival authority shifts the rest");
        let mut padded = settings_bytes(2, 3, 0, false);
        padded.extend([0u8; 60]);
        assert!(parse_settings(&padded).is_some());
        assert!(parse_settings(&settings_bytes(5, 3, 0, false)).is_none());
        assert!(parse_settings(&settings_bytes(0, 3, 0, false)).is_none());
        assert!(parse_settings(&multisig_bytes(2, 3, 0, false)).is_none(), "a v4 account is not v5 settings");
        let mut trailing = settings_bytes(2, 3, 0, false);
        trailing.push(1);
        assert!(parse_settings(&trailing).is_none());
    }

    #[test]
    fn v5_instructions_are_named_from_their_discriminators() {
        let disc = |name: &str| solana_sdk::hash::hashv(&[b"global:", name.as_bytes()]).to_bytes()[..8].to_vec();
        assert_eq!(instruction_name(V5, &disc("execute_transaction_sync")), Some("execute_transaction_sync"));
        assert_eq!(instruction_name(V5, &disc("approve_proposal")), Some("approve_proposal"));
        assert_eq!(instruction_name(V4, &disc("approve_proposal")), None, "v4 calls it proposal_approve");
        assert_eq!(name_of(V5), Some("Squads v5"));
    }

    #[test]
    fn vault_addresses_differ_by_index_and_version() {
        let ms = "11111111111111111111111111111112";
        let a = vault_address(V4, ms, 0).unwrap();
        assert_ne!(a, vault_address(V4, ms, 1).unwrap());
        assert_ne!(a, vault_address(V5, ms, 0).unwrap());
        assert!(vault_address("Other", ms, 0).is_none());
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
