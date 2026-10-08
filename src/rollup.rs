//! Hourly rollups of what a program did. The per-second window is memory-only
//! and short; summaries ("what happened in the last 24 hours") are built from
//! these, which are stored and survive restarts.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Most distinct instruction names and error labels kept per hour.
const MAX_KEYS: usize = 60;
const MAX_BIG_MOVES: usize = 5;
const PRECISION: u32 = 10;

/// Counts distinct wallets in 1 KB with about 3% error, and merges across hours
/// without keeping the wallets themselves.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hll {
    #[serde(with = "registers")]
    regs: Vec<u8>,
}

impl Default for Hll {
    fn default() -> Self {
        Self { regs: vec![0; 1 << PRECISION] }
    }
}

mod registers {
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        let v = base64::engine::general_purpose::STANDARD.decode(text).map_err(serde::de::Error::custom)?;
        if v.len() != 1 << super::PRECISION {
            return Err(serde::de::Error::custom("wrong register count"));
        }
        Ok(v)
    }
}

/// FNV-1a with a splitmix64 finish: stable across versions, unlike `DefaultHasher`.
fn hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h ^= h >> 30;
    h = h.wrapping_mul(0xbf58476d1ce4e5b9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94d049bb133111eb);
    h ^ (h >> 31)
}

impl Hll {
    pub fn add(&mut self, item: &str) {
        let h = hash(item);
        let idx = (h >> (64 - PRECISION)) as usize;
        // Position of the first set bit in the remaining bits, capped by their number.
        let rest = (h << PRECISION) | (1 << (PRECISION - 1));
        let rho = rest.leading_zeros() as u8 + 1;
        if rho > self.regs[idx] {
            self.regs[idx] = rho;
        }
    }

    pub fn merge(&mut self, other: &Hll) {
        for (a, b) in self.regs.iter_mut().zip(&other.regs) {
            *a = (*a).max(*b);
        }
    }

    pub fn count(&self) -> u64 {
        let m = self.regs.len() as f64;
        let sum: f64 = self.regs.iter().map(|r| 2f64.powi(-(*r as i32))).sum();
        let alpha = 0.7213 / (1.0 + 1.079 / m);
        let raw = alpha * m * m / sum;
        let zeros = self.regs.iter().filter(|r| **r == 0).count() as f64;
        let est = if raw <= 2.5 * m && zeros > 0.0 { m * (m / zeros).ln() } else { raw };
        est.round() as u64
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IxCount {
    pub tx: u64,
    pub failed: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ErrCount {
    pub count: u64,
    pub label: String,
}

/// A single large movement of value, kept for the summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BigMove {
    pub signature: String,
    pub at: i64,
    pub amount: f64,
    pub symbol: String,
    pub usd: Option<f64>,
}

impl BigMove {
    /// Ranking value: USD where known, otherwise the raw amount.
    fn weight(&self) -> f64 {
        self.usd.unwrap_or(self.amount)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Rollup {
    pub tx: u64,
    pub failed: u64,
    /// Lamports paid in fees.
    pub fees: u64,
    pub cu_sum: u64,
    pub cu_n: u64,
    /// Value moved by priced, liquid tokens (Blur) and SOL; a transaction with several hops counts each.
    pub usd_volume: f64,
    pub sol_volume: f64,
    /// Net USD change across the watched vaults (positive = money in).
    pub vault_net_usd: f64,
    pub peak_tps: f64,
    pub peak_at: i64,
    pub signers: Hll,
    pub instructions: HashMap<String, IxCount>,
    pub errors: HashMap<String, ErrCount>,
    pub largest: Vec<BigMove>,
}

/// What one transaction contributes.
pub struct Observation<'a> {
    pub ok: bool,
    pub fee: u64,
    pub compute_units: Option<u64>,
    pub signer: Option<&'a str>,
    pub instructions: &'a [String],
    pub error: Option<(&'a str, &'a str)>,
    pub usd: f64,
    pub sol: f64,
    pub big: Option<BigMove>,
}

impl Rollup {
    pub fn record(&mut self, o: Observation<'_>) {
        self.tx += 1;
        if !o.ok {
            self.failed += 1;
        }
        self.fees += o.fee;
        if let Some(cu) = o.compute_units {
            self.cu_sum += cu;
            self.cu_n += 1;
        }
        if let Some(s) = o.signer {
            self.signers.add(s);
        }
        for name in o.instructions {
            if let Some(c) = self.instructions.get_mut(name) {
                c.tx += 1;
                c.failed += (!o.ok) as u64;
            } else if self.instructions.len() < MAX_KEYS {
                self.instructions.insert(name.clone(), IxCount { tx: 1, failed: (!o.ok) as u64 });
            }
        }
        if let Some((key, label)) = o.error {
            if let Some(c) = self.errors.get_mut(key) {
                c.count += 1;
            } else if self.errors.len() < MAX_KEYS {
                self.errors.insert(key.to_string(), ErrCount { count: 1, label: label.to_string() });
            }
        }
        self.usd_volume += o.usd;
        self.sol_volume += o.sol;
        if let Some(big) = o.big {
            self.add_big(big);
        }
    }

    fn add_big(&mut self, big: BigMove) {
        self.largest.push(big);
        self.largest.sort_by(|a, b| b.weight().total_cmp(&a.weight()));
        self.largest.truncate(MAX_BIG_MOVES);
    }

    /// Notes the busiest second seen so far.
    pub fn note_tps(&mut self, tps: f64, at: i64) {
        if tps > self.peak_tps {
            self.peak_tps = tps;
            self.peak_at = at;
        }
    }

    pub fn merge(&mut self, other: &Rollup) {
        self.tx += other.tx;
        self.failed += other.failed;
        self.fees += other.fees;
        self.cu_sum += other.cu_sum;
        self.cu_n += other.cu_n;
        self.usd_volume += other.usd_volume;
        self.sol_volume += other.sol_volume;
        self.vault_net_usd += other.vault_net_usd;
        if other.peak_tps > self.peak_tps {
            self.peak_tps = other.peak_tps;
            self.peak_at = other.peak_at;
        }
        self.signers.merge(&other.signers);
        for (k, v) in &other.instructions {
            let c = self.instructions.entry(k.clone()).or_default();
            c.tx += v.tx;
            c.failed += v.failed;
        }
        for (k, v) in &other.errors {
            let c = self.errors.entry(k.clone()).or_default();
            c.count += v.count;
            if c.label.is_empty() {
                c.label = v.label.clone();
            }
        }
        for big in &other.largest {
            self.add_big(big.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs<'a>(ok: bool, signer: &'a str, ixs: &'a [String]) -> Observation<'a> {
        Observation { ok, fee: 5000, compute_units: Some(1000), signer: Some(signer), instructions: ixs, error: None, usd: 0.0, sol: 0.0, big: None }
    }

    #[test]
    fn hll_counts_distinct_wallets_and_merges() {
        let mut a = Hll::default();
        let mut b = Hll::default();
        for i in 0..5000 {
            a.add(&format!("wallet{i}"));
        }
        for i in 2500..7500 {
            b.add(&format!("wallet{i}"));
        }
        let within = |got: u64, want: f64| (got as f64 - want).abs() / want < 0.08;
        assert!(within(a.count(), 5000.0), "{}", a.count());
        a.merge(&b);
        assert!(within(a.count(), 7500.0), "union {}", a.count());
        // Repeats add nothing; tiny sets are near exact.
        let mut c = Hll::default();
        for _ in 0..100 {
            c.add("same");
        }
        assert_eq!(c.count(), 1);
        assert_eq!(Hll::default().count(), 0);
    }

    #[test]
    fn hll_survives_a_round_trip_through_json() {
        let mut h = Hll::default();
        for i in 0..300 {
            h.add(&format!("w{i}"));
        }
        let back: Hll = serde_json::from_str(&serde_json::to_string(&h).unwrap()).unwrap();
        assert_eq!(back.count(), h.count());
        assert!(serde_json::from_str::<Hll>(r#"{"regs":"AAAA"}"#).is_err(), "wrong size is rejected");
    }

    #[test]
    fn records_and_merges_hours() {
        let sell = vec!["Sell".to_string()];
        let buy = vec!["Buy".to_string()];
        let mut h1 = Rollup::default();
        h1.record(obs(true, "a", &buy));
        h1.record(obs(false, "b", &sell));
        h1.record(Observation { error: Some(("k", "TooLittleSolReceived in Pump.fun::Sell")), ..obs(false, "b", &sell) });
        h1.note_tps(7.0, 100);
        let mut h2 = Rollup::default();
        h2.record(Observation { usd: 1200.0, sol: 3.0, big: Some(BigMove { signature: "s".into(), at: 5, amount: 3.0, symbol: "SOL".into(), usd: Some(1200.0) }), ..obs(true, "c", &buy) });
        h2.note_tps(9.0, 200);

        h1.merge(&h2);
        assert_eq!((h1.tx, h1.failed), (4, 2));
        assert_eq!(h1.fees, 20_000);
        assert_eq!(h1.signers.count(), 3);
        assert_eq!(h1.instructions["Buy"].tx, 2);
        assert_eq!((h1.instructions["Sell"].tx, h1.instructions["Sell"].failed), (2, 2));
        assert_eq!(h1.errors["k"].count, 1);
        assert_eq!((h1.peak_tps, h1.peak_at), (9.0, 200));
        assert_eq!(h1.usd_volume, 1200.0);
        assert_eq!(h1.largest[0].signature, "s");
    }

    #[test]
    fn distinct_keys_are_bounded() {
        let mut r = Rollup::default();
        for i in 0..200 {
            let names = vec![format!("ix{i}")];
            r.record(obs(true, "a", &names));
        }
        assert_eq!(r.instructions.len(), MAX_KEYS);
        assert_eq!(r.tx, 200, "totals still count everything");
    }
}
