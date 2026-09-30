//! Turns whatever a user pastes into the programs worth watching: a program
//! address, a transaction signature or explorer link, or an upgrade-authority
//! address (which lists every program it can upgrade). Everything read here is
//! public, so nothing is signed and ownership isn't checked.

use anyhow::{anyhow, bail, Result};
use futures_util::{stream, StreamExt};
use serde::Serialize;
use solana_account_decoder::{UiAccountEncoding, UiDataSliceConfig};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_config::{RpcAccountInfoConfig, RpcProgramAccountsConfig};
use solana_client::rpc_filter::{Memcmp, RpcFilterType};
use solana_sdk::pubkey::Pubkey;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use std::str::FromStr;
use vortex::events::programs::known_name;
use vortex::events::VortexTransaction;

pub const UPGRADEABLE_LOADER: &str = "BPFLoaderUpgradeab1e11111111111111111111111";
const LOADERS: [&str; 4] = [
    "BPFLoader1111111111111111111111111111111111",
    "BPFLoader2111111111111111111111111111111111",
    UPGRADEABLE_LOADER,
    "LoaderV411111111111111111111111111111111111",
];
/// Programs every transaction touches; listed last so the interesting ones lead.
const INFRA: [&str; 7] = [
    "11111111111111111111111111111111",
    "ComputeBudget111111111111111111111111111111",
    "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
    "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb",
    "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL",
    "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr",
    "T1pyyaTNZsKv2WcRAB8oVnk93mLJw2XzjtVYqCsaHqt",
];
/// Most programs one authority lookup returns.
const MAX_FOUND: usize = 25;

#[derive(Debug, PartialEq)]
pub enum Query {
    Address(String),
    Signature(String),
}

/// Pulls an address or signature out of pasted text, including explorer links
/// like `https://solscan.io/tx/<sig>` or `https://explorer.solana.com/address/<key>`.
pub fn parse_query(input: &str) -> Result<Query> {
    let input = input.trim();
    if input.is_empty() {
        bail!("Paste a program address, a wallet address, or a transaction signature");
    }
    let candidate = if input.contains("://") || input.contains('/') {
        let path = input.split(['?', '#']).next().unwrap_or(input);
        path.trim_end_matches('/')
            .split('/')
            .rev()
            .find(|seg| looks_like_base58(seg))
            .ok_or_else(|| anyhow!("Couldn't find an address or signature in that link"))?
    } else {
        input
    };
    if !looks_like_base58(candidate) {
        bail!("That isn't a Solana address or transaction signature");
    }
    match bs58::decode(candidate).into_vec() {
        Ok(b) if b.len() == 32 => Ok(Query::Address(candidate.to_string())),
        Ok(b) if b.len() == 64 => Ok(Query::Signature(candidate.to_string())),
        _ => bail!("That isn't a valid Solana address or transaction signature"),
    }
}

fn looks_like_base58(s: &str) -> bool {
    (32..=88).contains(&s.len()) && s.chars().all(|c| c.is_ascii_alphanumeric() && !"0OIl".contains(c))
}

#[derive(Debug, Serialize, Clone)]
pub struct Candidate {
    pub program_id: String,
    pub name: Option<String>,
    /// Shared plumbing (System, Token, …) that nearly every transaction uses.
    pub infra: bool,
}

#[derive(Debug, Serialize, Clone)]
pub struct Resolution {
    /// "program", "authority", "transaction" or "account".
    pub kind: &'static str,
    /// The address or signature that was looked up.
    pub subject: String,
    pub headline: String,
    pub programs: Vec<Candidate>,
}

/// A ProgramData account never changes which program it belongs to, so this is kept for good.
/// Lookups that finish before a timeout still count toward the next attempt.
fn program_of() -> &'static Mutex<HashMap<Pubkey, Pubkey>> {
    static M: OnceLock<Mutex<HashMap<Pubkey, Pubkey>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

const RESULT_TTL: Duration = Duration::from_secs(600);
const MAX_CACHED: usize = 500;

fn results() -> &'static Mutex<HashMap<String, (Instant, Resolution)>> {
    static M: OnceLock<Mutex<HashMap<String, (Instant, Resolution)>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

fn candidate(id: &str) -> Candidate {
    Candidate {
        program_id: id.to_string(),
        name: known_name(id).map(String::from),
        infra: INFRA.contains(&id),
    }
}

/// The distinct programs a transaction invoked, application programs first.
pub fn programs_in(tx: &VortexTransaction) -> Vec<Candidate> {
    let mut seen = HashSet::new();
    let mut found: Vec<Candidate> = tx
        .instructions
        .iter()
        .filter(|i| seen.insert(i.program_id.clone()))
        .map(|i| {
            let mut c = candidate(&i.program_id);
            c.name = c.name.or_else(|| i.program_name.clone());
            c
        })
        .collect();
    found.sort_by_key(|c| c.infra);
    found
}

/// Memcmp filters that match ProgramData accounts whose upgrade authority is `authority`.
/// Layout: u32 variant (3) | u64 slot | Option tag (1) | 32-byte authority.
fn authority_filters(authority: &Pubkey) -> Vec<RpcFilterType> {
    vec![
        RpcFilterType::Memcmp(Memcmp::new_base58_encoded(0, &[3, 0, 0, 0])),
        RpcFilterType::Memcmp(Memcmp::new_base58_encoded(12, &[1])),
        RpcFilterType::Memcmp(Memcmp::new_base58_encoded(13, authority.as_ref())),
    ]
}

fn ids_only() -> RpcProgramAccountsConfig {
    RpcProgramAccountsConfig {
        filters: None,
        account_config: RpcAccountInfoConfig {
            encoding: Some(UiAccountEncoding::Base64),
            data_slice: Some(UiDataSliceConfig { offset: 0, length: 0 }),
            ..Default::default()
        },
        with_context: None,
    }
}

/// Programs found for an authority, and how many couldn't be resolved (rate limits, timeouts).
pub struct Owned {
    pub programs: Vec<Candidate>,
    pub missing: usize,
}

/// Every program whose upgrade authority is `authority`.
pub async fn programs_by_authority(rpc: &RpcClient, authority: &Pubkey) -> Result<Owned> {
    let loader = Pubkey::from_str(UPGRADEABLE_LOADER)?;
    let mut cfg = ids_only();
    cfg.filters = Some(authority_filters(authority));
    let data_accounts = rpc
        .get_program_accounts_with_config(&loader, cfg)
        .await
        .map_err(|e| anyhow!("Couldn't look up programs for that address: {e}"))?;

    // A Program account is [variant 2 | its ProgramData address]; find each by that pointer.
    // A few at a time with retries: public endpoints rate-limit bursts.
    let data_addrs: Vec<Pubkey> = data_accounts.into_iter().take(MAX_FOUND).map(|(k, _)| k).collect();
    let results: Vec<Option<Pubkey>> = stream::iter(data_addrs.into_iter().map(|data_addr| {
        async move {
            if let Some(p) = program_of().lock().unwrap().get(&data_addr) {
                return Some(*p);
            }
            for attempt in 0..3u64 {
                let mut cfg = ids_only();
                cfg.filters = Some(vec![
                    RpcFilterType::DataSize(36),
                    RpcFilterType::Memcmp(Memcmp::new_base58_encoded(4, data_addr.as_ref())),
                ]);
                if let Ok(v) = rpc.get_program_accounts_with_config(&loader, cfg).await {
                    let program = v.into_iter().next().map(|(program, _)| program);
                    if let Some(p) = program {
                        program_of().lock().unwrap().insert(data_addr, p);
                    }
                    return program;
                }
                tokio::time::sleep(std::time::Duration::from_millis(400 * (attempt + 1))).await;
            }
            None
        }
    }))
    .buffer_unordered(4)
    .collect()
    .await;
    let missing = results.iter().filter(|r| r.is_none()).count();
    let mut programs: Vec<Candidate> = results.into_iter().flatten().map(|p| candidate(&p.to_string())).collect();
    programs.sort_by(|a, b| b.name.is_some().cmp(&a.name.is_some()).then(a.program_id.cmp(&b.program_id)));
    Ok(Owned { programs, missing })
}

pub async fn resolve_address(rpc: &RpcClient, address: &str) -> Result<Resolution> {
    if let Some((at, r)) = results().lock().unwrap().get(address) {
        if at.elapsed() < RESULT_TTL {
            return Ok(r.clone());
        }
    }
    let r = lookup_address(rpc, address).await?;
    // Incomplete answers aren't kept, so searching again finishes the job.
    if !r.headline.contains("couldn't be loaded") {
        let mut cache = results().lock().unwrap();
        if cache.len() >= MAX_CACHED {
            cache.retain(|_, (at, _)| at.elapsed() < RESULT_TTL);
        }
        if cache.len() < MAX_CACHED {
            cache.insert(address.to_string(), (Instant::now(), r.clone()));
        }
    }
    Ok(r)
}

async fn lookup_address(rpc: &RpcClient, address: &str) -> Result<Resolution> {
    let key = Pubkey::from_str(address)?;
    let account = rpc.get_account(&key).await.ok();
    if let Some(a) = &account {
        if a.executable && LOADERS.contains(&a.owner.to_string().as_str()) {
            let c = candidate(address);
            return Ok(Resolution {
                kind: "program",
                subject: address.into(),
                headline: format!("{} is a program", c.name.as_deref().unwrap_or("This address")),
                programs: vec![c],
            });
        }
    }
    let owned = programs_by_authority(rpc, &key).await?;
    if !owned.programs.is_empty() {
        let n = owned.programs.len();
        let mut headline = format!("{n} program{} upgradeable by this address", if n == 1 { "" } else { "s" });
        if owned.missing > 0 {
            headline.push_str(&format!(" ({} more couldn't be loaded; search again to retry)", owned.missing));
        }
        return Ok(Resolution { kind: "authority", subject: address.into(), headline, programs: owned.programs });
    }
    if account.is_none() {
        bail!("Nothing found at that address on mainnet");
    }
    Ok(Resolution {
        kind: "account",
        subject: address.into(),
        headline: "No programs found for this address. It can still be watched as an account.".into(),
        programs: vec![candidate(address)],
    })
}

pub fn resolve_transaction(signature: &str, tx: &VortexTransaction) -> Resolution {
    let programs = programs_in(tx);
    let apps = programs.iter().filter(|p| !p.infra).count();
    Resolution {
        kind: "transaction",
        subject: signature.into(),
        headline: format!(
            "This transaction called {} program{}{}",
            programs.len(),
            if programs.len() == 1 { "" } else { "s" },
            if apps == 0 { "" } else { "; pick the ones you want to watch" }
        ),
        programs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PUMP: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";

    #[test]
    fn plain_address() {
        assert_eq!(parse_query(&format!("  {PUMP}\n")).unwrap(), Query::Address(PUMP.into()));
    }

    #[test]
    fn explorer_links() {
        let url = format!("https://explorer.solana.com/address/{PUMP}?cluster=mainnet-beta");
        assert_eq!(parse_query(&url).unwrap(), Query::Address(PUMP.into()));
        let sig = bs58::encode([7u8; 64]).into_string();
        let url = format!("https://solscan.io/tx/{sig}#accounts");
        assert_eq!(parse_query(&url).unwrap(), Query::Signature(sig));
    }

    #[test]
    fn rejects_junk() {
        assert!(parse_query("").is_err());
        assert!(parse_query("hello world").is_err());
        assert!(parse_query("https://example.com/nothing-here").is_err());
        // Right alphabet, wrong length once decoded.
        assert!(parse_query(&"1".repeat(40)).is_err());
    }

    #[test]
    fn authority_filter_layout() {
        let f = authority_filters(&Pubkey::new_unique());
        assert_eq!(f.len(), 3);
    }
}
