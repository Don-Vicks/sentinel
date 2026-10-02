//! Turns one `VortexTransaction` into an investigation view: who took part,
//! where value moved, which accounts changed and a plain-language narrative.
//! Account owners are resolved through the same RPC Vortex uses, so vaults
//! and PDAs are labelled by the program that controls them.

use crate::analyze::{short, symbol_for};
use crate::idl::{DecodedInstruction, IdlError, IdlRegistry};
use base64::Engine;
use crate::pricing::PriceBook;
use serde::Serialize;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;
use std::sync::Mutex;
use vortex::events::{logs::Invocation, programs, TransferKind, VortexTransaction};

#[derive(Debug, Clone, Serialize)]
pub struct Trace {
    pub parties: Vec<Party>,
    pub flows: Vec<Flow>,
    pub balance_changes: Vec<BalanceChange>,
    pub state_changes: Vec<StateChange>,
    pub call_tree: Vec<CallNode>,
    pub narrative: Vec<String>,
    /// Transfers in a failed transaction executed and were then rolled back.
    pub reverted: bool,
    /// Instructions decoded with the program's on-chain Anchor IDL.
    pub decoded: Vec<DecodedView>,
    /// The failing program's own description of a custom error code.
    pub error_detail: Option<IdlError>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DecodedView {
    pub path: String,
    pub program_id: String,
    pub idl_name: Option<String>,
    #[serde(flatten)]
    pub instruction: DecodedInstruction,
}

#[derive(Debug, Clone, Serialize)]
pub struct Party {
    pub address: String,
    pub label: String,
    pub roles: Vec<String>,
    pub owner_program: Option<String>,
    pub owner_program_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Flow {
    pub from: String,
    pub to: String,
    pub amount: f64,
    pub symbol: String,
    pub mint: Option<String>,
    pub kind: TransferKind,
    pub instruction: String,
    pub from_account: Option<String>,
    pub to_account: Option<String>,
    /// Value at the Solami Blur last-trade price, if priced.
    pub usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BalanceChange {
    pub owner: String,
    pub symbol: String,
    pub mint: Option<String>,
    pub delta: f64,
    pub usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StateChange {
    pub account: String,
    pub owner: Option<String>,
    pub kind: &'static str,
    pub symbol: String,
    pub before: f64,
    pub after: f64,
    pub delta: f64,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CallNode {
    pub index: usize,
    pub parent: Option<usize>,
    pub depth: u32,
    pub program_id: String,
    pub program_name: Option<String>,
    pub instruction: Option<String>,
    pub compute_consumed: Option<u64>,
    pub success: Option<bool>,
    pub failure: Option<String>,
    pub logs: Vec<String>,
}

/// Remembers which program owns an account; owners rarely change.
#[derive(Default)]
pub struct OwnerCache {
    owners: Mutex<HashMap<String, Option<(String, bool)>>>,
}

impl OwnerCache {
    async fn resolve(&self, rpc: &RpcClient, keys: &[String]) -> HashMap<String, (String, bool)> {
        let missing: Vec<String> = {
            let owners = self.owners.lock().unwrap();
            keys.iter().filter(|k| !owners.contains_key(*k)).cloned().collect()
        };
        for chunk in missing.chunks(100) {
            let pubkeys: Vec<Pubkey> =
                chunk.iter().filter_map(|k| Pubkey::from_str(k).ok()).collect();
            match rpc.get_multiple_accounts(&pubkeys).await {
                Ok(accounts) => {
                    let mut owners = self.owners.lock().unwrap();
                    for (pk, acct) in pubkeys.iter().zip(accounts) {
                        owners.insert(
                            pk.to_string(),
                            acct.map(|a| (a.owner.to_string(), a.executable)),
                        );
                    }
                }
                Err(e) => tracing::debug!(error = %e, "owner lookup failed"),
            }
        }
        let owners = self.owners.lock().unwrap();
        keys.iter()
            .filter_map(|k| owners.get(k).cloned().flatten().map(|v| (k.clone(), v)))
            .collect()
    }
}

pub fn program_label(program_id: &str, monitored: &HashMap<String, String>) -> String {
    monitored
        .get(program_id)
        .cloned()
        .or_else(|| programs::known_name(program_id).map(str::to_string))
        .or_else(|| crate::catalog::name_of(program_id).map(str::to_string))
        .unwrap_or_else(|| short(program_id))
}

pub async fn build(
    tx: &VortexTransaction,
    monitored: &HashMap<String, String>,
    rpc: Option<&RpcClient>,
    cache: &OwnerCache,
    prices: &PriceBook,
    idls: Option<&std::sync::Arc<IdlRegistry>>,
) -> Trace {
    let (decoded, error_detail) = match idls {
        Some(idls) => decode_with_idls(tx, idls).await,
        None => (vec![], None),
    };
    let flows: Vec<Flow> = tx
        .transfers
        .iter()
        .map(|t| {
            let from = match t.kind {
                TransferKind::Mint => format!("mint:{}", t.mint.clone().unwrap_or_default()),
                _ => t.from_owner.clone().or_else(|| t.from.clone()).unwrap_or_default(),
            };
            let to = match t.kind {
                TransferKind::Burn => "burn".to_string(),
                _ => t.to_owner.clone().or_else(|| t.to.clone()).unwrap_or_default(),
            };
            Flow {
                from,
                to,
                amount: t.amount,
                symbol: symbol_for(t.mint.as_deref()),
                mint: t.mint.clone(),
                kind: t.kind.clone(),
                instruction: t.instruction.clone(),
                from_account: t.from.clone(),
                to_account: t.to.clone(),
                usd: prices.usd(t.mint.as_deref(), t.amount),
            }
        })
        .filter(|f| f.amount > 0.0)
        .collect();

    // Everyone who sent, received, signed or changed balance.
    let mut addresses: Vec<String> = Vec::new();
    let mut add = |a: &str| {
        if !a.is_empty() && !addresses.iter().any(|x| x == a) {
            addresses.push(a.to_string());
        }
    };
    tx.signers().for_each(&mut add);
    for f in &flows {
        add(&f.from);
        add(&f.to);
    }
    for b in &tx.token_balances {
        if let Some(o) = &b.owner {
            add(o);
        }
    }

    let lookup: Vec<String> = addresses
        .iter()
        .filter(|a| !a.contains(':') && *a != "burn")
        .cloned()
        .collect();
    let owners = match rpc {
        Some(rpc) => cache.resolve(rpc, &lookup).await,
        None => HashMap::new(),
    };

    let fee_payer = tx.fee_payer().unwrap_or_default().to_string();
    let parties: Vec<Party> = addresses
        .iter()
        .map(|a| party(a, tx, &fee_payer, monitored, owners.get(a)))
        .collect();
    let label_of: HashMap<&str, &str> = parties
        .iter()
        .map(|p| (p.address.as_str(), p.label.as_str()))
        .collect();
    let name = |a: &str| label_of.get(a).map(|s| s.to_string()).unwrap_or_else(|| short(a));

    let balance_changes = balance_changes(tx, prices);
    let state_changes = state_changes(tx);
    let call_tree = call_tree(&tx.invocations, monitored);
    let narrative = narrative(tx, &flows, &call_tree, monitored, &name);

    Trace {
        parties,
        flows,
        balance_changes,
        state_changes,
        call_tree,
        narrative,
        reverted: !tx.success && !tx.transfers.is_empty(),
        decoded,
        error_detail,
    }
}

async fn decode_with_idls(
    tx: &VortexTransaction,
    idls: &std::sync::Arc<IdlRegistry>,
) -> (Vec<DecodedView>, Option<IdlError>) {
    // Native programs have no Anchor IDL; skip the lookups.
    let mut programs: Vec<&str> = tx
        .instructions
        .iter()
        .map(|i| i.program_id.as_str())
        .filter(|p| !is_native(p))
        .collect();
    programs.sort();
    programs.dedup();
    let fetched = futures_util::future::join_all(programs.iter().map(|p| idls.get(p))).await;
    let by_program: HashMap<&str, _> = programs
        .iter()
        .zip(fetched)
        .filter_map(|(p, idl)| Some((*p, idl?)))
        .collect();

    let decoded = tx
        .instructions
        .iter()
        .filter_map(|ix| {
            let idl = by_program.get(ix.program_id.as_str())?;
            let data = base64::engine::general_purpose::STANDARD.decode(&ix.data).ok()?;
            Some(DecodedView {
                path: ix.path.clone(),
                program_id: ix.program_id.clone(),
                idl_name: idl.name.clone(),
                instruction: idl.decode_instruction(&data, &ix.accounts)?,
            })
        })
        .collect();

    let error_detail = crate::analyze::fingerprint(tx).and_then(|fp| {
        let code = fp.code?;
        let idl = by_program.get(fp.program_id.as_str())?;
        idl.error(code).cloned()
    });
    (decoded, error_detail)
}

fn is_native(program: &str) -> bool {
    matches!(
        program,
        programs::SYSTEM_PROGRAM
            | programs::TOKEN_PROGRAM
            | programs::TOKEN_2022_PROGRAM
            | programs::COMPUTE_BUDGET_PROGRAM
            | "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL"
            | "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr"
            | "Memo1UhkJRfHyvLMcVucJwxXeuD728EqVDDwQDxFMNo"
            | "Stake11111111111111111111111111111111111111"
            | "Vote111111111111111111111111111111111111111"
            | "AddressLookupTab1e1111111111111111111111111"
    )
}

fn party(
    address: &str,
    tx: &VortexTransaction,
    fee_payer: &str,
    monitored: &HashMap<String, String>,
    owner: Option<&(String, bool)>,
) -> Party {
    let mut roles = Vec::new();
    if address == fee_payer {
        roles.push("fee payer".to_string());
    }
    if tx.accounts.iter().any(|a| a.pubkey == address && a.signer) {
        roles.push("signer".to_string());
    }
    if monitored.contains_key(address) {
        roles.push("monitored program".to_string());
    }

    let owner_program = owner.map(|(o, _)| o.clone());
    let executable = owner.is_some_and(|(_, e)| *e);
    let owner_name = owner_program.as_deref().map(|o| program_label(o, monitored));

    let label = if let Some(mint) = address.strip_prefix("mint:") {
        format!("Mint ({})", symbol_for(Some(mint)))
    } else if address == "burn" {
        "Burned".to_string()
    } else if monitored.contains_key(address) || executable {
        program_label(address, monitored)
    } else if address == fee_payer {
        format!("Signer {}", short(address))
    } else if roles.iter().any(|r| r == "signer") {
        format!("Co-signer {}", short(address))
    } else {
        match owner_program.as_deref() {
            Some(programs::SYSTEM_PROGRAM) | None => format!("Wallet {}", short(address)),
            Some(programs::TOKEN_PROGRAM | programs::TOKEN_2022_PROGRAM) => {
                format!("Token account {}", short(address))
            }
            Some(o) => format!("{} account {}", program_label(o, monitored), short(address)),
        }
    };
    if owner_program
        .as_deref()
        .is_some_and(|o| monitored.contains_key(o))
    {
        roles.push("program-owned".to_string());
    }

    Party {
        address: address.to_string(),
        label,
        roles,
        owner_program,
        owner_program_name: owner_name,
    }
}

fn balance_changes(tx: &VortexTransaction, prices: &PriceBook) -> Vec<BalanceChange> {
    let mut out = Vec::new();
    for a in &tx.accounts {
        let delta = a.post_lamports as f64 - a.pre_lamports as f64;
        if delta != 0.0 && a.signer {
            out.push(BalanceChange {
                owner: a.pubkey.clone(),
                symbol: "SOL".into(),
                mint: None,
                delta: delta / 1e9,
                usd: prices.usd(None, delta / 1e9),
            });
        }
    }
    let mut by_owner: BTreeMap<(String, String), f64> = BTreeMap::new();
    for b in &tx.token_balances {
        if b.delta != 0.0 {
            let owner = b.owner.clone().unwrap_or_else(|| b.account.clone());
            *by_owner.entry((owner, b.mint.clone())).or_default() += b.delta;
        }
    }
    for ((owner, mint), delta) in by_owner {
        out.push(BalanceChange {
            owner,
            symbol: symbol_for(Some(&mint)),
            usd: prices.usd(Some(&mint), delta),
            mint: Some(mint),
            delta,
        });
    }
    out
}

fn state_changes(tx: &VortexTransaction) -> Vec<StateChange> {
    let mut out = Vec::new();
    for a in tx.accounts.iter().filter(|a| a.writable) {
        let token = tx.token_balances.iter().find(|b| b.account == a.pubkey);
        if let Some(b) = token {
            let note = if a.pre_lamports == 0 && a.post_lamports > 0 {
                Some("created".to_string())
            } else if a.pre_lamports > 0 && a.post_lamports == 0 {
                Some("closed".to_string())
            } else {
                None
            };
            out.push(StateChange {
                account: a.pubkey.clone(),
                owner: b.owner.clone(),
                kind: "token",
                symbol: symbol_for(Some(&b.mint)),
                before: b.pre,
                after: b.post,
                delta: b.delta,
                note,
            });
        } else if a.pre_lamports != a.post_lamports {
            let note = if a.pre_lamports == 0 {
                Some("created".to_string())
            } else if a.post_lamports == 0 {
                Some("closed".to_string())
            } else {
                None
            };
            out.push(StateChange {
                account: a.pubkey.clone(),
                owner: None,
                kind: "lamports",
                symbol: "SOL".into(),
                before: a.pre_lamports as f64 / 1e9,
                after: a.post_lamports as f64 / 1e9,
                delta: (a.post_lamports as f64 - a.pre_lamports as f64) / 1e9,
                note,
            });
        } else {
            out.push(StateChange {
                account: a.pubkey.clone(),
                owner: None,
                kind: "data",
                symbol: String::new(),
                before: 0.0,
                after: 0.0,
                delta: 0.0,
                note: Some("writable; account data may have changed".to_string()),
            });
        }
    }
    out
}

fn call_tree(invocations: &[Invocation], monitored: &HashMap<String, String>) -> Vec<CallNode> {
    invocations
        .iter()
        .enumerate()
        .map(|(i, inv)| CallNode {
            index: i,
            parent: inv.parent,
            depth: inv.depth,
            program_name: Some(program_label(&inv.program_id, monitored)),
            program_id: inv.program_id.clone(),
            instruction: inv.instruction.clone(),
            compute_consumed: inv.compute_consumed,
            success: inv.success,
            failure: inv.failure.clone(),
            logs: inv.logs.clone(),
        })
        .collect()
}

fn fmt_amount(x: f64) -> String {
    let a = x.abs();
    if a >= 1_000_000.0 {
        format!("{:.2}M", x / 1_000_000.0)
    } else if a >= 1_000.0 {
        format!("{:.1}K", x / 1_000.0)
    } else if a >= 1.0 {
        format!("{x:.3}")
    } else {
        format!("{x:.6}")
    }
}

fn usd_suffix(usd: Option<f64>) -> String {
    match usd {
        Some(u) if u >= 1_000_000.0 => format!(" (${:.2}M)", u / 1_000_000.0),
        Some(u) if u >= 10_000.0 => format!(" (${:.1}K)", u / 1_000.0),
        Some(u) if u >= 0.01 => format!(" (${u:.2})"),
        _ => String::new(),
    }
}

fn narrative(
    tx: &VortexTransaction,
    flows: &[Flow],
    calls: &[CallNode],
    monitored: &HashMap<String, String>,
    name: &dyn Fn(&str) -> String,
) -> Vec<String> {
    let mut lines = Vec::new();
    let payer = tx.fee_payer().map(name).unwrap_or_else(|| "Unknown signer".into());
    let top: Vec<String> = calls
        .iter()
        .filter(|c| c.depth == 1 && c.program_id != programs::COMPUTE_BUDGET_PROGRAM)
        .map(|c| match &c.instruction {
            Some(ix) => format!("{}::{}", c.program_name.as_deref().unwrap_or("?"), ix),
            None => c.program_name.clone().unwrap_or_default(),
        })
        .collect();
    if !top.is_empty() {
        lines.push(format!("{payer} called {}.", top.join(", then ")));
    }
    // Programs are often reached through a router; name the monitored ones.
    let mut via: Vec<String> = calls
        .iter()
        .filter(|c| c.depth > 1 && monitored.contains_key(&c.program_id))
        .map(|c| match &c.instruction {
            Some(ix) => format!("{}::{}", c.program_name.as_deref().unwrap_or("?"), ix),
            None => c.program_name.clone().unwrap_or_default(),
        })
        .collect();
    via.dedup();
    // Anchor event self-CPIs carry no instruction name; drop them when the
    // same program already appears with one.
    let named: Vec<String> = via.iter().filter_map(|v| v.split_once("::").map(|(p, _)| p.to_string())).collect();
    via.retain(|v| v.contains("::") || !named.contains(v));
    if !via.is_empty() {
        lines.push(format!("That invoked {} via CPI.", via.join(", ")));
    }

    for f in flows.iter().take(8) {
        let verb = match f.kind {
            TransferKind::Mint => "minted to",
            TransferKind::Burn => "burned from",
            _ => "→",
        };
        let line = match f.kind {
            TransferKind::Mint => format!("{} {} {} {}", fmt_amount(f.amount), f.symbol, verb, name(&f.to)),
            TransferKind::Burn => format!("{} {} {} {}", fmt_amount(f.amount), f.symbol, verb, name(&f.from)),
            _ => format!("{} {}{}: {} {} {}", fmt_amount(f.amount), f.symbol, usd_suffix(f.usd), name(&f.from), verb, name(&f.to)),
        };
        lines.push(line);
    }
    if flows.len() > 8 {
        lines.push(format!("…and {} more transfers.", flows.len() - 8));
    }

    if let Some(err) = &tx.error {
        let at = calls
            .iter()
            .filter(|c| c.success == Some(false))
            .max_by_key(|c| c.depth)
            .map(|c| match &c.instruction {
                Some(ix) => format!("{}::{}", c.program_name.as_deref().unwrap_or("?"), ix),
                None => c.program_name.clone().unwrap_or_default(),
            })
            .unwrap_or_else(|| "the runtime".into());
        lines.push(format!(
            "Failed in {at} with {}. Every change was rolled back; only the {} SOL fee was charged.",
            err.name.as_deref().unwrap_or(&err.message),
            fmt_amount(tx.fee as f64 / 1e9)
        ));
    }
    lines
}
