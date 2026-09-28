//! USD prices from Solami Blur (`POST /data/token/price`). Sentinel notes
//! every mint it sees move; a background task prices new mints within about a
//! second and refreshes known ones periodically, in batches of up to 1000.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::sync::Notify;

pub const WSOL: &str = "So11111111111111111111111111111111111111112";
/// Prices of tokens thinner than this are shown but never trigger USD alerts:
/// one trade can move them arbitrarily.
pub const MIN_TRUSTED_LIQUIDITY_USD: f64 = 10_000.0;
const REFRESH: Duration = Duration::from_secs(30);
const MAX_TRACKED: usize = 5_000;
const BATCH: usize = 1_000;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Price {
    pub usd: f64,
    pub liquidity_usd: f64,
}

impl Price {
    pub fn trusted(&self) -> bool {
        self.liquidity_usd >= MIN_TRUSTED_LIQUIDITY_USD
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct PricingStatus {
    pub enabled: bool,
    pub priced_mints: usize,
    pub tracked_mints: usize,
    pub last_refresh: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

pub struct PriceBook {
    prices: RwLock<HashMap<String, Price>>,
    tracked: Mutex<HashSet<String>>,
    pending: Mutex<HashSet<String>>,
    wake: Notify,
    status: Mutex<PricingStatus>,
}

#[derive(Deserialize)]
struct Row {
    mint: String,
    price_usd: Option<String>,
    liquidity_usd: Option<String>,
}

impl PriceBook {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            prices: RwLock::new(HashMap::new()),
            tracked: Mutex::new(HashSet::new()),
            pending: Mutex::new(HashSet::new()),
            wake: Notify::new(),
            status: Mutex::new(PricingStatus::default()),
        })
    }

    /// USD price for a mint; `None` means native SOL.
    pub fn get(&self, mint: Option<&str>) -> Option<Price> {
        let key = mint.unwrap_or(WSOL);
        let price = self.prices.read().unwrap().get(key).copied();
        // SOL is always deep enough to trust, whatever Blur reports as its
        // (venue-less) liquidity.
        price.map(|p| if key == WSOL { Price { liquidity_usd: f64::INFINITY, ..p } } else { p })
    }

    pub fn usd(&self, mint: Option<&str>, amount: f64) -> Option<f64> {
        self.get(mint).map(|p| p.usd * amount)
    }

    /// Records that a mint moved; unpriced mints are fetched promptly.
    pub fn note(&self, mint: Option<&str>) {
        let key = mint.unwrap_or(WSOL);
        let mut tracked = self.tracked.lock().unwrap();
        if tracked.contains(key) || tracked.len() >= MAX_TRACKED {
            return;
        }
        tracked.insert(key.to_string());
        self.pending.lock().unwrap().insert(key.to_string());
        self.wake.notify_one();
    }

    pub fn status(&self) -> PricingStatus {
        let mut s = self.status.lock().unwrap().clone();
        s.priced_mints = self.prices.read().unwrap().len();
        s.tracked_mints = self.tracked.lock().unwrap().len();
        s
    }

    pub fn spawn(self: &Arc<Self>, base_url: String, api_key: String) {
        self.status.lock().unwrap().enabled = true;
        for m in [WSOL, "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB"] {
            self.note(Some(m));
        }
        let this = self.clone();
        tokio::spawn(async move {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("reqwest client");
            let url = format!("{}/data/token/price", base_url.trim_end_matches('/'));
            let mut last_full = std::time::Instant::now() - REFRESH;
            loop {
                tokio::select! {
                    _ = this.wake.notified() => {
                        // Let a burst of new mints collect into one request.
                        tokio::time::sleep(Duration::from_millis(750)).await;
                    }
                    _ = tokio::time::sleep(REFRESH) => {}
                }
                let mints: Vec<String> = if last_full.elapsed() >= REFRESH {
                    last_full = std::time::Instant::now();
                    this.pending.lock().unwrap().clear();
                    this.tracked.lock().unwrap().iter().cloned().collect()
                } else {
                    this.pending.lock().unwrap().drain().collect()
                };
                for chunk in mints.chunks(BATCH) {
                    this.fetch(&client, &url, &api_key, chunk).await;
                }
            }
        });
    }

    async fn fetch(&self, client: &reqwest::Client, url: &str, key: &str, mints: &[String]) {
        let res = client
            .post(url)
            .header("x-api-key", key)
            .json(&serde_json::json!({ "chain": "solana", "addresses": mints, "liquidity": true }))
            .send()
            .await;
        let rows: Result<Vec<Row>, String> = match res {
            Ok(r) if r.status().is_success() => r.json().await.map_err(|e| e.to_string()),
            Ok(r) => Err(format!("Blur returned HTTP {}", r.status())),
            Err(e) => Err(e.to_string()),
        };
        let mut status = self.status.lock().unwrap();
        match rows {
            Ok(rows) => {
                let mut prices = self.prices.write().unwrap();
                for row in rows {
                    // Blur sends decimals as strings.
                    let Some(usd) = row.price_usd.and_then(|p| p.parse::<f64>().ok()) else { continue };
                    if !usd.is_finite() || usd <= 0.0 {
                        continue;
                    }
                    let liquidity_usd = row
                        .liquidity_usd
                        .and_then(|l| l.parse().ok())
                        .unwrap_or(0.0);
                    prices.insert(row.mint, Price { usd, liquidity_usd });
                }
                status.last_refresh = Some(Utc::now());
                status.last_error = None;
            }
            Err(e) => {
                tracing::warn!(error = %e, "Blur price refresh failed");
                status.last_error = Some(e);
            }
        }
    }

    /// Test/dev hook to seed a price without the network.
    pub fn insert(&self, mint: &str, usd: f64, liquidity_usd: f64) {
        self.prices
            .write()
            .unwrap()
            .insert(mint.to_string(), Price { usd, liquidity_usd });
    }
}
