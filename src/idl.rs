//! Anchor IDLs fetched from chain, used to name instruction accounts, decode
//! arguments and translate custom error codes. Supports the current IDL spec
//! (explicit discriminators) and the legacy format (sighash of the name).

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::{Map, Number, Value};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::str::FromStr;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

pub struct Idl {
    pub program_id: String,
    pub name: Option<String>,
    instructions: Vec<IdlInstruction>,
    errors: HashMap<u32, IdlError>,
    types: HashMap<String, Value>,
}

struct IdlInstruction {
    name: String,
    discriminator: Vec<u8>,
    accounts: Vec<String>,
    args: Vec<(String, Value)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct IdlError {
    pub code: u32,
    pub name: String,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NamedAccount {
    pub name: String,
    pub pubkey: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DecodedInstruction {
    pub name: String,
    pub args: Value,
    pub accounts: Vec<NamedAccount>,
    /// Arguments that could not be decoded (newer fields missing from older
    /// transactions, or types this decoder doesn't know).
    pub partial: bool,
}

/// Anchor stores the IDL at `create_with_seed(find_program_address([]), "anchor:idl")`.
pub fn idl_address(program: &Pubkey) -> Pubkey {
    let (base, _) = Pubkey::find_program_address(&[], program);
    Pubkey::create_with_seed(&base, "anchor:idl", program).expect("valid seed")
}

/// Account layout: 8-byte discriminator, 32-byte authority, u32 length,
/// zlib-compressed JSON.
pub fn decode_idl_account(data: &[u8]) -> Result<Value> {
    let len = u32::from_le_bytes(data.get(40..44).context("short IDL account")?.try_into()?) as usize;
    let compressed = data.get(44..44 + len).context("truncated IDL")?;
    let mut json = String::new();
    flate2::read::ZlibDecoder::new(compressed).read_to_string(&mut json)?;
    Ok(serde_json::from_str(&json)?)
}

fn snake(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn flatten_accounts(v: &Value, out: &mut Vec<String>) {
    for a in v.as_array().into_iter().flatten() {
        // Composite account groups nest their own list.
        if let Some(inner) = a.get("accounts") {
            flatten_accounts(inner, out);
        } else if let Some(name) = a["name"].as_str() {
            out.push(snake(name));
        }
    }
}

impl Idl {
    pub fn parse(program_id: &str, v: &Value) -> Result<Self> {
        let legacy = v.get("metadata").and_then(|m| m.get("spec")).is_none();
        let mut instructions = Vec::new();
        for ix in v["instructions"].as_array().context("no instructions")? {
            let name = ix["name"].as_str().context("unnamed instruction")?.to_string();
            let discriminator = match ix.get("discriminator").and_then(Value::as_array) {
                Some(d) => d.iter().filter_map(|b| b.as_u64().map(|b| b as u8)).collect(),
                None => solana_sdk::hash::hashv(&[b"global:", snake(&name).as_bytes()]).to_bytes()[..8].to_vec(),
            };
            let mut accounts = Vec::new();
            flatten_accounts(&ix["accounts"], &mut accounts);
            let args = ix["args"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|a| (a["name"].as_str().unwrap_or("?").to_string(), a["type"].clone()))
                .collect();
            instructions.push(IdlInstruction {
                name: if legacy { snake(&name) } else { name },
                discriminator,
                accounts,
                args,
            });
        }
        let errors = v["errors"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| {
                Some((
                    e["code"].as_u64()? as u32,
                    IdlError {
                        code: e["code"].as_u64()? as u32,
                        name: e["name"].as_str()?.to_string(),
                        message: e["msg"].as_str().map(str::to_string),
                    },
                ))
            })
            .collect();
        let types = v["types"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| Some((t["name"].as_str()?.to_string(), t["type"].clone())))
            .collect();
        Ok(Self {
            program_id: program_id.to_string(),
            name: v["metadata"]["name"].as_str().or(v["name"].as_str()).map(str::to_string),
            instructions,
            errors,
            types,
        })
    }

    pub fn error(&self, code: u32) -> Option<&IdlError> {
        self.errors.get(&code)
    }

    pub fn decode_instruction(&self, data: &[u8], accounts: &[String]) -> Option<DecodedInstruction> {
        let ix = self
            .instructions
            .iter()
            .find(|ix| !ix.discriminator.is_empty() && data.starts_with(&ix.discriminator))?;
        let mut cur = &data[ix.discriminator.len()..];
        let mut args = Map::new();
        let mut partial = false;
        for (name, ty) in &ix.args {
            match self.read(ty, &mut cur, 0) {
                Ok(v) => {
                    args.insert(name.clone(), v);
                }
                Err(_) => {
                    partial = true;
                    break;
                }
            }
        }
        let named = accounts
            .iter()
            .enumerate()
            .map(|(i, pubkey)| NamedAccount {
                name: ix.accounts.get(i).cloned().unwrap_or_else(|| format!("remaining_{}", i - ix.accounts.len())),
                pubkey: pubkey.clone(),
            })
            .collect();
        Some(DecodedInstruction {
            name: ix.name.clone(),
            args: Value::Object(args),
            accounts: named,
            partial,
        })
    }

    fn read(&self, ty: &Value, cur: &mut &[u8], depth: u8) -> Result<Value> {
        if depth > 16 {
            bail!("type nesting too deep");
        }
        let take = |cur: &mut &[u8], n: usize| -> Result<Vec<u8>> {
            if cur.len() < n {
                bail!("out of data");
            }
            let (a, b) = cur.split_at(n);
            *cur = b;
            Ok(a.to_vec())
        };
        // Integers above 2^53 don't survive JSON numbers; send them as strings.
        let int = |v: i128| -> Value {
            if v.unsigned_abs() <= (1u128 << 53) {
                Value::Number(Number::from(v as i64))
            } else {
                Value::String(v.to_string())
            }
        };
        if let Some(s) = ty.as_str() {
            return Ok(match s {
                "bool" => Value::Bool(take(cur, 1)?[0] != 0),
                "u8" => int(take(cur, 1)?[0] as i128),
                "i8" => int(take(cur, 1)?[0] as i8 as i128),
                "u16" => int(u16::from_le_bytes(take(cur, 2)?.try_into().unwrap()) as i128),
                "i16" => int(i16::from_le_bytes(take(cur, 2)?.try_into().unwrap()) as i128),
                "u32" => int(u32::from_le_bytes(take(cur, 4)?.try_into().unwrap()) as i128),
                "i32" => int(i32::from_le_bytes(take(cur, 4)?.try_into().unwrap()) as i128),
                "u64" => int(u64::from_le_bytes(take(cur, 8)?.try_into().unwrap()) as i128),
                "i64" => int(i64::from_le_bytes(take(cur, 8)?.try_into().unwrap()) as i128),
                "u128" => Value::String(u128::from_le_bytes(take(cur, 16)?.try_into().unwrap()).to_string()),
                "i128" => Value::String(i128::from_le_bytes(take(cur, 16)?.try_into().unwrap()).to_string()),
                "f32" => serde_json::json!(f32::from_le_bytes(take(cur, 4)?.try_into().unwrap())),
                "f64" => serde_json::json!(f64::from_le_bytes(take(cur, 8)?.try_into().unwrap())),
                "pubkey" | "publicKey" => Value::String(bs58::encode(take(cur, 32)?).into_string()),
                "string" => {
                    let n = u32::from_le_bytes(take(cur, 4)?.try_into().unwrap()) as usize;
                    Value::String(String::from_utf8_lossy(&take(cur, n)?).into_owned())
                }
                "bytes" => {
                    let n = u32::from_le_bytes(take(cur, 4)?.try_into().unwrap()) as usize;
                    Value::String(bs58::encode(take(cur, n)?).into_string())
                }
                other => bail!("unsupported type {other}"),
            });
        }
        if let Some(inner) = ty.get("option").or_else(|| ty.get("coption")) {
            let tag_len = if ty.get("coption").is_some() { 4 } else { 1 };
            let tag = take(cur, tag_len)?;
            return if tag.iter().all(|b| *b == 0) {
                Ok(Value::Null)
            } else {
                self.read(inner, cur, depth + 1)
            };
        }
        if let Some(inner) = ty.get("vec") {
            let n = u32::from_le_bytes(take(cur, 4)?.try_into().unwrap()) as usize;
            if n > 10_000 {
                bail!("implausible vec length");
            }
            return (0..n).map(|_| self.read(inner, cur, depth + 1)).collect::<Result<Vec<_>>>().map(Value::Array);
        }
        if let Some(arr) = ty.get("array").and_then(Value::as_array) {
            let n = arr.get(1).and_then(Value::as_u64).context("array length")? as usize;
            if arr[0] == "u8" {
                return Ok(Value::String(bs58::encode(take(cur, n)?).into_string()));
            }
            return (0..n).map(|_| self.read(&arr[0], cur, depth + 1)).collect::<Result<Vec<_>>>().map(Value::Array);
        }
        if let Some(def) = ty.get("defined") {
            let name = def.as_str().or_else(|| def["name"].as_str()).context("defined name")?;
            let t = self.types.get(name).ok_or_else(|| anyhow!("unknown type {name}"))?.clone();
            return self.read_defined(&t, cur, depth + 1);
        }
        bail!("unsupported type {ty}")
    }

    fn read_defined(&self, t: &Value, cur: &mut &[u8], depth: u8) -> Result<Value> {
        match t["kind"].as_str() {
            Some("struct") => self.read_fields(&t["fields"], cur, depth),
            Some("enum") => {
                if cur.is_empty() {
                    bail!("out of data");
                }
                let tag = cur[0] as usize;
                *cur = &cur[1..];
                let variant = t["variants"].get(tag).context("enum tag out of range")?;
                let name = variant["name"].as_str().unwrap_or("?").to_string();
                if variant.get("fields").is_none() {
                    return Ok(Value::String(name));
                }
                let fields = self.read_fields(&variant["fields"], cur, depth)?;
                Ok(serde_json::json!({ name: fields }))
            }
            Some("type") | Some("alias") => self.read(&t["alias"], cur, depth),
            _ => bail!("unsupported defined kind"),
        }
    }

    fn read_fields(&self, fields: &Value, cur: &mut &[u8], depth: u8) -> Result<Value> {
        let list = fields.as_array().cloned().unwrap_or_default();
        // Named fields are objects with a name; tuple fields are bare types.
        if list.iter().all(|f| f.get("name").is_some()) {
            let mut m = Map::new();
            for f in &list {
                m.insert(f["name"].as_str().unwrap_or("?").to_string(), self.read(&f["type"], cur, depth)?);
            }
            Ok(Value::Object(m))
        } else {
            list.iter().map(|f| self.read(f, cur, depth)).collect::<Result<Vec<_>>>().map(Value::Array)
        }
    }
}

/// Fetches and caches IDLs per program (including "has no IDL").
pub struct IdlRegistry {
    rpc: Option<Arc<RpcClient>>,
    cache: RwLock<HashMap<String, Option<Arc<Idl>>>>,
    inflight: Mutex<HashSet<String>>,
}

impl IdlRegistry {
    pub fn new(rpc: Option<Arc<RpcClient>>) -> Arc<Self> {
        Arc::new(Self {
            rpc,
            cache: RwLock::new(HashMap::new()),
            inflight: Mutex::new(HashSet::new()),
        })
    }

    /// Already-fetched IDL, without waiting on the network.
    pub fn cached(&self, program: &str) -> Option<Arc<Idl>> {
        self.cache.read().unwrap().get(program).cloned().flatten()
    }

    pub fn insert(&self, idl: Idl) {
        self.cache.write().unwrap().insert(idl.program_id.clone(), Some(Arc::new(idl)));
    }

    /// Starts a background fetch if this program hasn't been looked up yet.
    pub fn request(self: &Arc<Self>, program: &str) {
        if self.rpc.is_none() || self.cache.read().unwrap().contains_key(program) {
            return;
        }
        if !self.inflight.lock().unwrap().insert(program.to_string()) {
            return;
        }
        let this = self.clone();
        let program = program.to_string();
        tokio::spawn(async move {
            this.load(&program).await;
        });
    }

    /// Returns the IDL, fetching it (bounded wait) if needed.
    pub async fn get(self: &Arc<Self>, program: &str) -> Option<Arc<Idl>> {
        if let Some(entry) = self.cache.read().unwrap().get(program) {
            return entry.clone();
        }
        tokio::time::timeout(Duration::from_secs(4), self.load(program)).await.ok().flatten()
    }

    async fn load(&self, program: &str) -> Option<Arc<Idl>> {
        let result = self.fetch(program).await;
        let idl = match result {
            Ok(idl) => {
                tracing::info!(program, name = ?idl.name, "loaded Anchor IDL");
                Some(Arc::new(idl))
            }
            Err(e) => {
                tracing::debug!(program, error = %e, "no IDL");
                None
            }
        };
        self.cache.write().unwrap().insert(program.to_string(), idl.clone());
        self.inflight.lock().unwrap().remove(program);
        idl
    }

    async fn fetch(&self, program: &str) -> Result<Idl> {
        let rpc = self.rpc.as_ref().context("no RPC configured")?;
        let pk = Pubkey::from_str(program)?;
        let data = rpc.get_account_data(&idl_address(&pk)).await?;
        Idl::parse(program, &decode_idl_account(&data)?)
    }
}
