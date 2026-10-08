//! A program's code and authority, as opposed to its traffic: when it is
//! upgraded, who may upgrade it, and whether that is a single key or something
//! program-controlled. Upgrades and authority changes are read from the
//! upgradeable loader's instructions (top-level or via CPI, e.g. a multisig
//! executing a vote); the current state comes from the ProgramData account.

use crate::model::Severity;
use crate::resolve::UPGRADEABLE_LOADER;
use anyhow::{anyhow, bail, Result};
use base64::Engine;
use serde::Serialize;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use vortex::events::VortexTransaction;

/// Squads v4 and v3: multisigs commonly used as upgrade authority.
pub const SQUADS_V4: &str = "SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf";
pub const SQUADS_V3: &str = "SMPLecH534NA9acpos4G6x7uf3LWbCAwZQE9e8ZekMu";

/// The ProgramData account an upgradeable program's code lives in.
pub fn programdata_address(program_id: &str) -> Option<String> {
    let program = Pubkey::from_str(program_id).ok()?;
    let loader = Pubkey::from_str(UPGRADEABLE_LOADER).ok()?;
    Some(Pubkey::find_program_address(&[program.as_ref()], &loader).0.to_string())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    /// New code was deployed from `buffer`.
    Upgrade { buffer: Option<String> },
    /// The upgrade authority changed. `None` makes the program immutable.
    SetAuthority { new_authority: Option<String> },
    /// The program account was closed.
    Close { recipient: Option<String> },
    /// More space was allocated for the code.
    Extend { additional_bytes: u32 },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AuthorityEvent {
    #[serde(flatten)]
    pub action: Action,
    pub program_id: String,
    pub programdata: String,
    /// The signer that authorised it (for a multisig, its vault PDA).
    pub authority: Option<String>,
    pub signature: String,
    pub slot: u64,
    /// Instruction path in the transaction; "3.1" means it ran via CPI.
    pub path: String,
    /// The Squads multisig call that executed it, when that is what ran it.
    pub via: Option<crate::squads::Via>,
    /// What that multisig requires to act, filled in once it has been read from chain.
    pub multisig: Option<crate::squads::MultisigInfo>,
}

impl AuthorityEvent {
    pub fn severity(&self) -> Severity {
        match &self.action {
            Action::Upgrade { .. } => Severity::High,
            Action::SetAuthority { new_authority: Some(_) } => Severity::Critical,
            Action::SetAuthority { new_authority: None } => Severity::High,
            Action::Close { .. } => Severity::Critical,
            Action::Extend { .. } => Severity::Low,
        }
    }

    pub fn headline(&self) -> &'static str {
        match &self.action {
            Action::Upgrade { .. } => "Program upgraded",
            Action::SetAuthority { new_authority: Some(_) } => "Upgrade authority changed",
            Action::SetAuthority { new_authority: None } => "Program made immutable",
            Action::Close { .. } => "Program closed",
            Action::Extend { .. } => "Program extended",
        }
    }

    /// True when the loader was called from another program (for example a
    /// multisig executing an approved upgrade) rather than directly.
    pub fn via_cpi(&self) -> bool {
        self.path.contains('.')
    }

    /// Who did it: the Squads multisig and member when one ran it, else the signing key.
    pub fn executed_by(&self) -> String {
        match &self.via {
            Some(v) => format!(
                "{} multisig {}{}{}",
                v.program,
                short(&v.multisig),
                v.executor.as_deref().map(|e| format!(" (member {})", short(e))).unwrap_or_default(),
                self.multisig.as_ref().map(|m| format!(", {}", m.describe())).unwrap_or_default()
            ),
            None => self.authority.as_deref().map(short).unwrap_or_else(|| "unknown".into()),
        }
    }

    pub fn summary(&self) -> String {
        let who = self.executed_by();
        match &self.action {
            Action::Upgrade { buffer } => format!(
                "New code deployed{} by {who}{}",
                buffer.as_deref().map(|b| format!(" from buffer {}", short(b))).unwrap_or_default(),
                if self.via_cpi() && self.via.is_none() { " (via CPI, e.g. a multisig vote)" } else { "" }
            ),
            Action::SetAuthority { new_authority: Some(new) } => {
                format!("Upgrade authority moved from {who} to {}", short(new))
            }
            Action::SetAuthority { new_authority: None } => {
                format!("{who} removed the upgrade authority; the program can no longer be upgraded")
            }
            Action::Close { .. } => format!("{who} closed the program account"),
            Action::Extend { additional_bytes } => format!("Program data extended by {additional_bytes} bytes"),
        }
    }

    pub fn explanation(&self) -> String {
        let what = match &self.action {
            Action::Upgrade { .. } => {
                "Whoever holds the upgrade authority can replace the program's code at any time, so every upgrade is \
                 worth confirming against your release process."
            }
            Action::SetAuthority { new_authority: Some(_) } => {
                "Control over future upgrades has moved to a different address. If you did not do this, the new \
                 holder can replace the program's code."
            }
            Action::SetAuthority { new_authority: None } => {
                "The program is now immutable. This is irreversible; confirm it was intended."
            }
            Action::Close { .. } => "A closed program stops executing, and its address can no longer be used.",
            Action::Extend { .. } => "Extension usually precedes an upgrade to larger code.",
        };
        format!(
            "Transaction {} (slot {}) called the upgradeable loader on {}. {what}",
            short(&self.signature),
            self.slot,
            short(&self.program_id)
        )
    }
}

fn short(s: &str) -> String {
    if s.len() > 10 {
        format!("{}…{}", &s[..4], &s[s.len() - 4..])
    } else {
        s.to_string()
    }
}

/// Decodes one upgradeable-loader instruction if it concerns this program.
/// Layout (bincode): u32 variant, then fields; accounts as in the loader's docs.
pub fn decode(
    data: &[u8],
    accounts: &[String],
    program_id: &str,
    programdata: &str,
) -> Option<(Action, Option<String>)> {
    let tag = u32::from_le_bytes(data.get(0..4)?.try_into().ok()?);
    let acc = |i: usize| accounts.get(i).cloned();
    let is = |i: usize, key: &str| accounts.get(i).is_some_and(|a| a == key);
    match tag {
        // Upgrade: [programdata, program, buffer, spill, rent, clock, authority]
        3 if is(1, program_id) || is(0, programdata) => Some((Action::Upgrade { buffer: acc(2) }, acc(6))),
        // SetAuthority: [programdata, current authority, new authority?]
        // SetAuthorityChecked: [programdata, current authority, new authority (signer)]
        4 | 7 if is(0, programdata) => Some((Action::SetAuthority { new_authority: acc(2) }, acc(1))),
        // Close: [closed account, recipient, authority, program?]
        5 if is(0, programdata) || is(3, program_id) => Some((Action::Close { recipient: acc(1) }, acc(2))),
        // ExtendProgram: [programdata, program, system program?, payer?]
        6 if is(0, programdata) => {
            let bytes = u32::from_le_bytes(data.get(4..8)?.try_into().ok()?);
            Some((Action::Extend { additional_bytes: bytes }, None))
        }
        _ => None,
    }
}

/// Upgrades and authority changes this transaction made to `program_id`.
/// Failed transactions changed nothing and are ignored.
pub fn detect(tx: &VortexTransaction, program_id: &str, programdata: &str) -> Vec<AuthorityEvent> {
    if !tx.success {
        return Vec::new();
    }
    tx.instructions
        .iter()
        .filter(|ix| ix.program_id == UPGRADEABLE_LOADER)
        .filter_map(|ix| {
            let data = base64::engine::general_purpose::STANDARD.decode(&ix.data).ok()?;
            let (action, authority) = decode(&data, &ix.accounts, program_id, programdata)?;
            Some(AuthorityEvent {
                action,
                program_id: program_id.to_string(),
                programdata: programdata.to_string(),
                authority,
                signature: tx.signature.clone(),
                slot: tx.slot,
                path: ix.path.clone(),
                via: crate::squads::executed_by(tx, &ix.path),
                multisig: None,
            })
        })
        .collect()
}

// ------------------------------------------------------------ current state

/// Who can change a program, read from chain.
#[derive(Debug, Clone, Serialize)]
pub struct Posture {
    pub program_id: String,
    pub programdata: Option<String>,
    /// Upgradeable programs only; false for programs on the older, immutable loaders.
    pub upgradeable: bool,
    /// `None` when the program is immutable.
    pub authority: Option<String>,
    /// "none", "single_key" (a wallet can upgrade it alone) or "program_controlled"
    /// (a PDA: a multisig vault, a DAO, ...).
    pub authority_kind: &'static str,
    pub last_deployed_slot: Option<u64>,
    pub code_bytes: Option<usize>,
    /// What is known to hold the authority, from upgrades Sentinel has watched it make.
    pub controller: Option<Controller>,
    pub risks: Vec<Risk>,
}

/// A Squads multisig seen executing upgrades with this program's authority.
#[derive(Debug, Clone, Serialize)]
pub struct Controller {
    pub name: String,
    pub multisig: String,
    pub requires: Option<crate::squads::MultisigInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Risk {
    pub level: Severity,
    pub text: String,
}

pub async fn fetch(rpc: &RpcClient, program_id: &str) -> Result<Posture> {
    let program = Pubkey::from_str(program_id).map_err(|_| anyhow!("Not a valid program address"))?;
    let account = rpc.get_account(&program).await.map_err(|_| anyhow!("Program account not found on this cluster"))?;
    if !account.executable {
        bail!("That address is not a program");
    }
    if account.owner.to_string() != UPGRADEABLE_LOADER {
        let mut p = posture("none", None, None, None, None);
        p.program_id = program_id.to_string();
        p.upgradeable = false;
        p.risks.push(Risk {
            level: Severity::Low,
            text: format!("Deployed with a non-upgradeable loader ({}), so its code cannot change.", short(&account.owner.to_string())),
        });
        return Ok(p);
    }
    // Program account: u32 variant (2) | ProgramData address.
    let pd_bytes: [u8; 32] = account.data.get(4..36).and_then(|b| b.try_into().ok()).ok_or_else(|| anyhow!("Unexpected program account layout"))?;
    let programdata = Pubkey::new_from_array(pd_bytes);
    let data = rpc.get_account(&programdata).await.map_err(|_| anyhow!("ProgramData account not found"))?;
    parse_programdata(program_id, &programdata.to_string(), &data.data)
}

/// ProgramData: u32 variant (3) | u64 slot | Option<Pubkey> authority | code.
pub fn parse_programdata(program_id: &str, programdata: &str, data: &[u8]) -> Result<Posture> {
    if data.len() < 13 || u32::from_le_bytes(data[0..4].try_into()?) != 3 {
        bail!("Unexpected ProgramData layout");
    }
    let slot = u64::from_le_bytes(data[4..12].try_into()?);
    let (authority, code_start) = match data[12] {
        0 => (None, 13),
        1 => {
            let key: [u8; 32] = data.get(13..45).and_then(|b| b.try_into().ok()).ok_or_else(|| anyhow!("Truncated ProgramData"))?;
            (Some(Pubkey::new_from_array(key)), 45)
        }
        _ => bail!("Unexpected ProgramData layout"),
    };
    let kind = match authority {
        None => "none",
        // A PDA has no private key: something on chain decides, not one person.
        Some(a) if !a.is_on_curve() => "program_controlled",
        Some(_) => "single_key",
    };
    let mut p = posture(kind, authority.map(|a| a.to_string()), Some(slot), Some(data.len().saturating_sub(code_start)), Some(programdata.to_string()));
    p.program_id = program_id.to_string();
    Ok(p)
}

fn posture(kind: &'static str, authority: Option<String>, slot: Option<u64>, code_bytes: Option<usize>, programdata: Option<String>) -> Posture {
    let mut risks = Vec::new();
    match kind {
        "single_key" => risks.push(Risk {
            level: Severity::High,
            text: "A single wallet can replace this program's code. If that key leaks, the program can be rewritten. \
                   Move the upgrade authority to a multisig."
                .into(),
        }),
        "program_controlled" => risks.push(Risk {
            level: Severity::Low,
            text: "The upgrade authority is program-controlled (for example a multisig vault). Upgrades need that \
                   program's approval flow."
                .into(),
        }),
        "none" => {}
        _ => {}
    }
    if authority.is_none() && kind == "none" && programdata.is_some() {
        risks.push(Risk { level: Severity::Low, text: "Immutable: the upgrade authority was removed, so the code can never change.".into() });
    }
    Posture {
        program_id: String::new(),
        programdata,
        upgradeable: true,
        authority,
        authority_kind: kind,
        last_deployed_slot: slot,
        code_bytes,
        controller: None,
        risks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::signer::Signer;

    const PROGRAM: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";

    fn pd() -> String {
        programdata_address(PROGRAM).unwrap()
    }

    fn accs(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn programdata_is_a_pda_of_the_loader() {
        let key = Pubkey::from_str(&pd()).unwrap();
        assert!(!key.is_on_curve(), "a PDA has no private key");
        assert_ne!(pd(), PROGRAM);
        assert!(programdata_address("not a key").is_none());
    }

    #[test]
    fn upgrade_names_the_buffer_and_authority() {
        let a = accs(&[&pd(), PROGRAM, "Buffer1111", "Spill1111", "Rent", "Clock", "Authority1111"]);
        let (action, authority) = decode(&3u32.to_le_bytes(), &a, PROGRAM, &pd()).unwrap();
        assert_eq!(action, Action::Upgrade { buffer: Some("Buffer1111".into()) });
        assert_eq!(authority.as_deref(), Some("Authority1111"));
    }

    #[test]
    fn set_authority_with_and_without_a_new_key() {
        let moved = accs(&[&pd(), "Old", "New"]);
        let (action, who) = decode(&4u32.to_le_bytes(), &moved, PROGRAM, &pd()).unwrap();
        assert_eq!(action, Action::SetAuthority { new_authority: Some("New".into()) });
        assert_eq!(who.as_deref(), Some("Old"));
        let frozen = accs(&[&pd(), "Old"]);
        let (action, _) = decode(&4u32.to_le_bytes(), &frozen, PROGRAM, &pd()).unwrap();
        assert_eq!(action, Action::SetAuthority { new_authority: None });
        let checked = decode(&7u32.to_le_bytes(), &moved, PROGRAM, &pd()).unwrap().0;
        assert_eq!(checked, Action::SetAuthority { new_authority: Some("New".into()) });
    }

    #[test]
    fn extend_reads_the_byte_count_and_other_programs_are_ignored() {
        let mut data = 6u32.to_le_bytes().to_vec();
        data.extend(2048u32.to_le_bytes());
        let (action, _) = decode(&data, &accs(&[&pd(), PROGRAM]), PROGRAM, &pd()).unwrap();
        assert_eq!(action, Action::Extend { additional_bytes: 2048 });
        // Some other program's upgrade, and a plain buffer write, are not ours.
        assert!(decode(&3u32.to_le_bytes(), &accs(&["OtherData", "OtherProgram", "B", "S", "R", "C", "A"]), PROGRAM, &pd()).is_none());
        assert!(decode(&1u32.to_le_bytes(), &accs(&["Buffer", "Authority"]), PROGRAM, &pd()).is_none());
        assert!(decode(&[1, 2], &[], PROGRAM, &pd()).is_none());
    }

    #[test]
    fn severity_reflects_what_was_lost() {
        let ev = |action| AuthorityEvent {
            action,
            program_id: PROGRAM.into(),
            programdata: pd(),
            authority: Some("Auth11111111111".into()),
            signature: "5".repeat(64),
            slot: 1,
            path: "0".into(),
            via: None,
            multisig: None,
        };
        assert_eq!(ev(Action::Upgrade { buffer: None }).severity(), Severity::High);
        assert_eq!(ev(Action::SetAuthority { new_authority: Some("N".into()) }).severity(), Severity::Critical);
        assert_eq!(ev(Action::SetAuthority { new_authority: None }).severity(), Severity::High);
        assert_eq!(ev(Action::Close { recipient: None }).severity(), Severity::Critical);
        assert!(ev(Action::Upgrade { buffer: None }).summary().contains("New code deployed"));
    }

    fn programdata_bytes(authority: Option<[u8; 32]>) -> Vec<u8> {
        let mut d = 3u32.to_le_bytes().to_vec();
        d.extend(123_456u64.to_le_bytes());
        match authority {
            Some(a) => {
                d.push(1);
                d.extend(a);
            }
            None => d.push(0),
        }
        d.extend([0u8; 100]);
        d
    }

    #[test]
    fn reads_authority_and_classifies_it() {
        // A wallet key (on the curve) is a single point of failure.
        let wallet = solana_sdk::signature::Keypair::new();
        let p = parse_programdata(PROGRAM, &pd(), &programdata_bytes(Some(wallet.pubkey().to_bytes()))).unwrap();
        assert_eq!(p.authority_kind, "single_key");
        assert_eq!(p.last_deployed_slot, Some(123_456));
        assert_eq!(p.code_bytes, Some(100));
        assert!(p.risks.iter().any(|r| r.level == Severity::High));

        // A PDA (here, the programdata address itself) is program-controlled.
        let pda = Pubkey::from_str(&pd()).unwrap();
        let p = parse_programdata(PROGRAM, &pd(), &programdata_bytes(Some(pda.to_bytes()))).unwrap();
        assert_eq!(p.authority_kind, "program_controlled");

        let frozen = parse_programdata(PROGRAM, &pd(), &programdata_bytes(None)).unwrap();
        assert_eq!(frozen.authority_kind, "none");
        assert!(frozen.authority.is_none());
        assert!(parse_programdata(PROGRAM, &pd(), &[1, 2, 3]).is_err());
    }
}
