//! Solami Beam, read side: how a transaction landed, and which transfers were
//! Beam tips. Both lookups are public and need no key. Sending through Beam
//! (QUIC with a swQoS certificate, tip of at least 100,000 lamports) is a
//! separate, funded path that Sentinel doesn't use.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use vortex::events::{TransferKind, VortexTransaction};

const DEFAULT_BASE: &str = "https://api.solami.dev";
const TIPS_TTL: Duration = Duration::from_secs(600);
const FOUND_TTL: Duration = Duration::from_secs(300);
/// A signature Beam hasn't seen yet may show up seconds later.
const MISSING_TTL: Duration = Duration::from_secs(20);
const MAX_CACHED: usize = 2_000;

/// Beam's account of a transaction it carried. Every field is optional so a
/// schema change on their side degrades the display instead of breaking it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Landing {
    pub is_landed: bool,
    pub landed_via_jito: bool,
    pub rebroadcasted: bool,
    pub region: Option<String>,
    pub tip_lamports: Option<u64>,
    pub tip_address: Option<String>,
    pub bundle_uuid: Option<String>,
    pub first_seen_ms: Option<u64>,
    pub forwarded_ms: Option<u64>,
}

impl Landing {
    /// Milliseconds from Beam receiving the transaction to forwarding it on.
    pub fn forward_latency_ms(&self) -> Option<u64> {
        match (self.first_seen_ms, self.forwarded_ms) {
            (Some(a), Some(b)) if b >= a => Some(b - a),
            _ => None,
        }
    }
}

/// SOL tipped to Beam inside one transaction.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Tip {
    pub lamports: u64,
    pub address: String,
}

pub struct BeamClient {
    http: reqwest::Client,
    base: String,
    tips: Mutex<Option<(Instant, HashSet<String>)>>,
    landings: Mutex<HashMap<String, (Instant, Option<Landing>)>>,
    /// Lookups answered by Solami (not the cache), and how many were Beam transactions.
    asked: AtomicU64,
    carried: AtomicU64,
}

impl BeamClient {
    pub fn new() -> Self {
        Self::with_base(std::env::var("BEAM_API_URL").unwrap_or_else(|_| DEFAULT_BASE.into()))
    }

    pub fn with_base(base: String) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(3))
                .build()
                .expect("reqwest client"),
            base: base.trim_end_matches('/').to_string(),
            tips: Mutex::new(None),
            landings: Mutex::new(HashMap::new()),
            asked: AtomicU64::new(0),
            carried: AtomicU64::new(0),
        }
    }

    /// Beam's tip accounts. Falls back to the last known list if the fetch fails.
    pub async fn tip_addresses(&self) -> HashSet<String> {
        let stale = {
            let g = self.tips.lock().unwrap();
            match &*g {
                Some((at, set)) if at.elapsed() < TIPS_TTL => return set.clone(),
                Some((_, set)) => set.clone(),
                None => HashSet::new(),
            }
        };
        let fetched = async {
            let r = self.http.get(format!("{}/onchain/tip-addresses", self.base)).send().await.ok()?;
            r.error_for_status().ok()?.json::<Vec<String>>().await.ok()
        }
        .await;
        match fetched {
            Some(list) => {
                let set: HashSet<String> = list.into_iter().collect();
                *self.tips.lock().unwrap() = Some((Instant::now(), set.clone()));
                set
            }
            None => stale,
        }
    }

    /// How Beam delivered `signature`; `None` if it wasn't sent through Beam
    /// (or the lookup failed).
    pub async fn landing(&self, signature: &str) -> Option<Landing> {
        if let Some((at, hit)) = self.landings.lock().unwrap().get(signature) {
            let ttl = if hit.is_some() { FOUND_TTL } else { MISSING_TTL };
            if at.elapsed() < ttl {
                return hit.clone();
            }
        }
        let res = self.http.get(format!("{}/swqos/tx/{signature}", self.base)).send().await.ok()?;
        self.asked.fetch_add(1, Ordering::Relaxed);
        // A 404 means "not a Beam transaction"; anything else unexpected isn't cached.
        let result = if res.status().as_u16() == 404 {
            None
        } else {
            Some(res.error_for_status().ok()?.json::<Landing>().await.ok()?)
        };
        if result.is_some() {
            self.carried.fetch_add(1, Ordering::Relaxed);
        }
        let mut cache = self.landings.lock().unwrap();
        if cache.len() >= MAX_CACHED {
            cache.retain(|_, (at, _)| at.elapsed() < FOUND_TTL);
        }
        if cache.len() < MAX_CACHED {
            cache.insert(signature.to_string(), (Instant::now(), result.clone()));
        }
        result
    }

    /// (lookups made, of which were sent through Beam)
    pub fn counts(&self) -> (u64, u64) {
        (self.asked.load(Ordering::Relaxed), self.carried.load(Ordering::Relaxed))
    }

    /// The tip this transaction paid to Beam, if any.
    pub async fn tip_in(&self, tx: &VortexTransaction) -> Option<Tip> {
        let tips = self.tip_addresses().await;
        tip_from(tx, &tips)
    }
}

impl Default for BeamClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Sums native transfers to Beam tip accounts.
pub fn tip_from(tx: &VortexTransaction, tips: &HashSet<String>) -> Option<Tip> {
    let mut total = 0u64;
    let mut address = None;
    for t in tx.transfers.iter().filter(|t| matches!(t.kind, TransferKind::Sol)) {
        if let Some(to) = t.to.as_ref().filter(|to| tips.contains(*to)) {
            total = total.saturating_add(t.amount_raw);
            address.get_or_insert_with(|| to.clone());
        }
    }
    address.map(|address| Tip { lamports: total, address })
}

/// One-line description for the investigation narrative.
pub fn describe(landing: Option<&Landing>, tip: Option<&Tip>) -> Option<String> {
    let sol = |l: u64| format!("{}", l as f64 / 1e9);
    match (landing, tip) {
        (Some(l), _) => {
            let via = if l.landed_via_jito { "through Jito" } else { "directly to a leader" };
            let mut s = format!("Sent through Solami Beam, landed {via}");
            if let Some(r) = &l.region {
                s.push_str(&format!(" from {r}"));
            }
            if let Some(ms) = l.forward_latency_ms() {
                s.push_str(&format!(", forwarded {ms} ms after Beam received it"));
            }
            if let Some(t) = l.tip_lamports.or(tip.map(|t| t.lamports)) {
                s.push_str(&format!(", tip {} SOL", sol(t)));
            }
            if !l.is_landed {
                s.push_str(". Beam has not recorded it landing");
            }
            Some(s + ".")
        }
        (None, Some(t)) => Some(format!("Tipped {} SOL to a Solami Beam tip account.", sol(t.lamports))),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landing_tolerates_missing_and_extra_fields() {
        let l: Landing = serde_json::from_str(
            r#"{"signature":"x","is_landed":true,"landed_via_jito":false,"region":"NYC","tip_lamports":100000,
                "first_seen_ms":1000,"forwarded_ms":1012,"unknown_future_field":1}"#,
        )
        .unwrap();
        assert!(l.is_landed);
        assert_eq!(l.forward_latency_ms(), Some(12));
        let empty: Landing = serde_json::from_str("{}").unwrap();
        assert!(!empty.is_landed && empty.forward_latency_ms().is_none());
    }

    #[test]
    fn narrative_covers_each_case() {
        let l = Landing { is_landed: true, landed_via_jito: true, region: Some("FRA".into()), tip_lamports: Some(100_000), ..Default::default() };
        let s = describe(Some(&l), None).unwrap();
        assert!(s.contains("through Jito") && s.contains("FRA") && s.contains("0.0001 SOL"), "{s}");
        let tip = Tip { lamports: 200_000, address: "t".into() };
        assert!(describe(None, Some(&tip)).unwrap().contains("0.0002 SOL"));
        assert!(describe(None, None).is_none());
    }
}
