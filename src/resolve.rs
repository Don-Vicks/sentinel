//! Turns whatever a user pastes into the programs worth watching: a program
//! address, a transaction signature or explorer link, or an upgrade-authority
//! address (which lists every program it can upgrade). Everything read here is
//! public, so nothing is signed and ownership isn't checked.

use anyhow::{anyhow, bail, Result};
use serde::Serialize;
use serde_json::{json, Value};
use solana_client::nonblocking::rpc_client::RpcClient;
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
    /// Programs still being resolved in the background; ask again for the rest.
    #[serde(skip_serializing_if = "is_zero")]
    pub pending: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// A ProgramData account never changes which program it belongs to, so every answer is kept
/// for good. Finding the owner is a scan of the whole loader (about 15 s on Solami, with no
/// index to speed it up), so each one runs in the background and requests wait on a budget.
enum Lookup {
    Pending,
    Done(Pubkey),
    Failed(Instant),
}

fn lookups() -> &'static Mutex<HashMap<Pubkey, Lookup>> {
    static M: OnceLock<Mutex<HashMap<Pubkey, Lookup>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

/// How long one request waits for owner lookups before returning what it has.
pub const RESPONSE_BUDGET: Duration = Duration::from_secs(5);
const RETRY_FAILED_AFTER: Duration = Duration::from_secs(60);
/// Pages one owner lookup may walk before giving up.
const MAX_PAGES: usize = 80;

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

fn b58(bytes: &[u8]) -> String {
    bs58::encode(bytes).into_string()
}

/// Memcmp filters that match ProgramData accounts whose upgrade authority is `authority`.
/// Layout: u32 variant (3) | u64 slot | Option tag (1) | 32-byte authority.
fn authority_filters(authority: &Pubkey) -> Value {
    json!([
        { "memcmp": { "offset": 0, "bytes": b58(&[3, 0, 0, 0]) } },
        { "memcmp": { "offset": 12, "bytes": b58(&[1]) } },
        { "memcmp": { "offset": 13, "bytes": b58(authority.as_ref()) } },
    ])
}

/// A Program account is [variant 2 | its ProgramData address]; match on that pointer.
fn owner_filters(program_data: &Pubkey) -> Value {
    json!([
        { "dataSize": 36 },
        { "memcmp": { "offset": 4, "bytes": b58(program_data.as_ref()) } },
    ])
}

fn http() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| reqwest::Client::builder().timeout(Duration::from_secs(40)).build().expect("reqwest client"))
}

async fn rpc_call(url: &str, method: &str, params: Value) -> Result<Value> {
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    let res: Value = http().post(url).json(&body).send().await?.json().await.map_err(|e| anyhow!("RPC reply wasn't JSON: {e}"))?;
    if let Some(err) = res.get("error") {
        bail!(
            "RPC error {}: {}",
            err.get("code").map(|c| c.to_string()).unwrap_or_default(),
            err.get("message").and_then(|m| m.as_str()).unwrap_or("unknown")
        );
    }
    Ok(res["result"].clone())
}

/// Addresses of every account under the upgradeable loader matching `filters`, no data.
/// Uses `getProgramAccountsV2` with pagination (Solami refuses the plain call on a program
/// this large and returns partial pages that must be followed) and falls back to the plain
/// call where V2 doesn't exist, as on the public RPC.
async fn matching_accounts(url: &str, filters: &Value) -> Result<Vec<Pubkey>> {
    let mut out = Vec::new();
    let mut key: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut cfg = json!({
            "encoding": "base64",
            "dataSlice": { "offset": 0, "length": 0 },
            "filters": filters,
            "limit": 1000,
        });
        if let Some(k) = &key {
            cfg["paginationKey"] = json!(k);
        }
        match rpc_call(url, "getProgramAccountsV2", json!([UPGRADEABLE_LOADER, cfg.clone()])).await {
            Ok(result) => {
                let page = result.get("value").unwrap_or(&result);
                for a in page["accounts"].as_array().into_iter().flatten() {
                    if let Some(k) = a["pubkey"].as_str().and_then(|k| Pubkey::from_str(k).ok()) {
                        out.push(k);
                    }
                }
                match page["paginationKey"].as_str() {
                    Some(k) if !k.is_empty() => key = Some(k.to_string()),
                    _ => return Ok(out),
                }
            }
            Err(e) if key.is_none() && e.to_string().contains("-32601") => {
                // No V2 on this endpoint: the plain call returns everything at once.
                cfg.as_object_mut().unwrap().remove("limit");
                let result = rpc_call(url, "getProgramAccounts", json!([UPGRADEABLE_LOADER, cfg])).await?;
                let list = result.as_array().or_else(|| result["value"].as_array());
                return Ok(list
                    .into_iter()
                    .flatten()
                    .filter_map(|a| a["pubkey"].as_str().and_then(|k| Pubkey::from_str(k).ok()))
                    .collect());
            }
            Err(e) => return Err(e),
        }
    }
    bail!("The lookup didn't finish within {MAX_PAGES} pages")
}

/// Starts (once) the background search for the program that owns `program_data`.
fn start_owner_lookup(url: &str, program_data: Pubkey) {
    {
        let mut map = lookups().lock().unwrap();
        match map.get(&program_data) {
            Some(Lookup::Done(_) | Lookup::Pending) => return,
            Some(Lookup::Failed(at)) if at.elapsed() < RETRY_FAILED_AFTER => return,
            _ => {}
        }
        map.insert(program_data, Lookup::Pending);
    }
    let url = url.to_string();
    tokio::spawn(async move {
        let mut outcome = Lookup::Failed(Instant::now());
        for attempt in 0..3u64 {
            match matching_accounts(&url, &owner_filters(&program_data)).await {
                Ok(found) => {
                    if let Some(p) = found.first() {
                        outcome = Lookup::Done(*p);
                    }
                    break;
                }
                Err(e) => {
                    tracing::debug!(error = %e, "owner lookup failed");
                    tokio::time::sleep(Duration::from_millis(500 * (attempt + 1))).await;
                }
            }
        }
        lookups().lock().unwrap().insert(program_data, outcome);
    });
}

/// Programs found for an authority. `pending` are still being resolved in the background;
/// `missing` failed and will be retried on a later request.
pub struct Owned {
    pub programs: Vec<Candidate>,
    pub pending: usize,
    pub missing: usize,
}

/// Every program whose upgrade authority is `authority`. Waits up to `budget` for the
/// per-program lookups, then returns what has resolved.
pub async fn programs_by_authority(rpc_url: &str, authority: &Pubkey, budget: Duration) -> Result<Owned> {
    let mut data_accounts = matching_accounts(rpc_url, &authority_filters(authority))
        .await
        .map_err(|e| anyhow!("Couldn't look up programs for that address: {e}"))?;
    data_accounts.truncate(MAX_FOUND);
    for d in &data_accounts {
        start_owner_lookup(rpc_url, *d);
    }

    let deadline = Instant::now() + budget;
    loop {
        let (programs, pending, missing) = {
            let map = lookups().lock().unwrap();
            let mut programs = Vec::new();
            let (mut pending, mut missing) = (0, 0);
            for d in &data_accounts {
                match map.get(d) {
                    Some(Lookup::Done(p)) => programs.push(candidate(&p.to_string())),
                    Some(Lookup::Failed(_)) => missing += 1,
                    _ => pending += 1,
                }
            }
            (programs, pending, missing)
        };
        if pending == 0 || Instant::now() >= deadline {
            let mut programs = programs;
            programs.sort_by(|a, b| b.name.is_some().cmp(&a.name.is_some()).then(a.program_id.cmp(&b.program_id)));
            return Ok(Owned { programs, pending, missing });
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

pub async fn resolve_address(rpc: &RpcClient, address: &str) -> Result<Resolution> {
    if let Some((at, r)) = results().lock().unwrap().get(address) {
        if at.elapsed() < RESULT_TTL {
            return Ok(r.clone());
        }
    }
    let r = lookup_address(rpc, address).await?;
    // Incomplete answers aren't kept, so searching again finishes the job.
    if r.pending == 0 && !r.headline.contains("couldn't be loaded") {
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
                pending: 0,
            });
        }
    }
    let owned = programs_by_authority(&rpc.url(), &key, RESPONSE_BUDGET).await?;
    let total = owned.programs.len() + owned.pending + owned.missing;
    if total > 0 {
        let mut headline = format!("{total} program{} upgradeable by this address", if total == 1 { "" } else { "s" });
        if owned.pending > 0 {
            headline.push_str(&format!(" ({} of {total} resolved so far)", owned.programs.len()));
        } else if owned.missing > 0 {
            headline.push_str(&format!(" ({} more couldn't be loaded; search again to retry)", owned.missing));
        }
        return Ok(Resolution { kind: "authority", subject: address.into(), headline, programs: owned.programs, pending: owned.pending });
    }
    if account.is_none() {
        bail!("Nothing found at that address on mainnet");
    }
    Ok(Resolution {
        kind: "account",
        subject: address.into(),
        headline: "No programs found for this address. It can still be watched as an account.".into(),
        programs: vec![candidate(address)],
        pending: 0,
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
        pending: 0,
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
        let key = Pubkey::new_unique();
        let f = authority_filters(&key);
        let f = f.as_array().unwrap();
        assert_eq!(f.len(), 3);
        assert_eq!(f[2]["memcmp"]["offset"], 13);
        assert_eq!(f[2]["memcmp"]["bytes"], key.to_string());
        assert_eq!(owner_filters(&key)[0]["dataSize"], 36);
    }
}
