//! Rolling per-second aggregates for one program.

use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};

/// Keep 15 minutes of 1-second buckets.
pub const HISTORY_SECS: i64 = 900;

#[derive(Debug, Default, Clone)]
pub struct Bucket {
    pub second: i64,
    pub tx: u32,
    pub failed: u32,
    pub cu_sum: u64,
    pub cu_n: u32,
    pub cu_max: u64,
    pub fees: u64,
    pub signers: HashSet<String>,
    pub fingerprints: HashMap<String, u32>,
    /// Bitmask of detectors that considered this second anomalous; such
    /// seconds are left out of that detector's future baselines.
    pub anomalous: u8,
}

pub const ANOMALY_FAILURE: u8 = 1;
pub const ANOMALY_ACTIVITY: u8 = 2;
pub const ANOMALY_COMPUTE: u8 = 4;

#[derive(Debug, Default, Clone)]
pub struct Stats {
    pub seconds: u32,
    pub tx: u64,
    pub failed: u64,
    pub cu_sum: u64,
    pub cu_n: u64,
    pub cu_max: u64,
    pub fees: u64,
    pub unique_signers: usize,
    pub fingerprints: HashMap<String, u64>,
}

impl Stats {
    pub fn failure_rate(&self) -> f64 {
        if self.tx == 0 {
            0.0
        } else {
            self.failed as f64 * 100.0 / self.tx as f64
        }
    }

    pub fn tps(&self) -> f64 {
        if self.seconds == 0 {
            0.0
        } else {
            self.tx as f64 / self.seconds as f64
        }
    }

    pub fn avg_cu(&self) -> f64 {
        if self.cu_n == 0 {
            0.0
        } else {
            self.cu_sum as f64 / self.cu_n as f64
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SeriesPoint {
    /// Unix seconds.
    pub t: i64,
    pub tx: u32,
    pub failed: u32,
    pub avg_cu: f64,
    pub max_cu: u64,
}

#[derive(Default)]
pub struct Window {
    buckets: VecDeque<Bucket>,
    /// First second any data was recorded; used for warmup.
    pub first_second: Option<i64>,
    pub total_tx: u64,
    pub total_failed: u64,
}

impl Window {
    /// Ensures a bucket exists for every second up to `now`.
    pub fn advance(&mut self, now: i64) {
        let next = self.buckets.back().map(|b| b.second + 1).unwrap_or(now);
        for second in next.max(now - HISTORY_SECS + 1)..=now {
            self.buckets.push_back(Bucket {
                second,
                ..Default::default()
            });
        }
        while self
            .buckets
            .front()
            .is_some_and(|b| b.second <= now - HISTORY_SECS)
        {
            self.buckets.pop_front();
        }
    }

    pub fn record(
        &mut self,
        second: i64,
        success: bool,
        compute: Option<u64>,
        fee: u64,
        signer: Option<&str>,
        fingerprint: Option<String>,
    ) {
        self.advance(second);
        self.first_second.get_or_insert(second);
        self.total_tx += 1;
        // Transactions are bucketed by arrival, so they land in the newest
        // bucket unless the clock went backwards.
        let Some(bucket) = self.buckets.iter_mut().rev().find(|b| b.second <= second) else {
            return;
        };
        bucket.tx += 1;
        if !success {
            bucket.failed += 1;
            self.total_failed += 1;
        }
        if let Some(cu) = compute {
            bucket.cu_sum += cu;
            bucket.cu_n += 1;
            bucket.cu_max = bucket.cu_max.max(cu);
        }
        bucket.fees += fee;
        if let Some(s) = signer {
            if !bucket.signers.contains(s) {
                bucket.signers.insert(s.to_string());
            }
        }
        if let Some(fp) = fingerprint {
            *bucket.fingerprints.entry(fp).or_default() += 1;
        }
    }

    /// Seconds of history available (for warmup checks).
    pub fn age(&self, now: i64) -> i64 {
        self.first_second.map(|f| now - f).unwrap_or(0)
    }

    /// Marks `[now - from_ago, now)` as anomalous for the given detector bit.
    pub fn flag(&mut self, now: i64, from_ago: i64, bit: u8) {
        for b in self.buckets.iter_mut().filter(|b| b.second >= now - from_ago && b.second < now) {
            b.anomalous |= bit;
        }
    }

    /// Aggregates seconds in `[now - from_ago, now - to_ago)`.
    pub fn stats(&self, now: i64, from_ago: i64, to_ago: i64) -> Stats {
        self.stats_masked(now, from_ago, to_ago, 0)
    }

    /// Like `stats`, skipping seconds flagged with any bit in `mask`.
    pub fn stats_masked(&self, now: i64, from_ago: i64, to_ago: i64, mask: u8) -> Stats {
        let (start, end) = (now - from_ago, now - to_ago);
        let mut s = Stats::default();
        let mut signers: HashSet<&str> = HashSet::new();
        for b in self
            .buckets
            .iter()
            .filter(|b| b.second >= start && b.second < end && b.anomalous & mask == 0)
        {
            s.seconds += 1;
            s.tx += b.tx as u64;
            s.failed += b.failed as u64;
            s.cu_sum += b.cu_sum;
            s.cu_n += b.cu_n as u64;
            s.cu_max = s.cu_max.max(b.cu_max);
            s.fees += b.fees;
            signers.extend(b.signers.iter().map(String::as_str));
            for (k, v) in &b.fingerprints {
                *s.fingerprints.entry(k.clone()).or_default() += *v as u64;
            }
        }
        s.unique_signers = signers.len();
        s
    }

    /// Transaction counts per `chunk`-second slice over `[now - from_ago, now - to_ago)`.
    /// Chunks containing a second flagged with `mask` are skipped.
    pub fn chunk_counts(&self, now: i64, from_ago: i64, to_ago: i64, chunk: i64, mask: u8) -> Vec<f64> {
        let (start, end) = (now - from_ago, now - to_ago);
        let mut out = Vec::new();
        let mut cursor = start;
        while cursor + chunk <= end {
            let in_chunk = || self.buckets.iter().filter(|b| b.second >= cursor && b.second < cursor + chunk);
            if in_chunk().any(|b| b.anomalous & mask != 0) {
                cursor += chunk;
                continue;
            }
            let n: u32 = in_chunk().map(|b| b.tx).sum();
            out.push(n as f64);
            cursor += chunk;
        }
        out
    }

    /// Completed seconds only (excludes `now`).
    pub fn series(&self, now: i64, secs: i64) -> Vec<SeriesPoint> {
        self.buckets
            .iter()
            .filter(|b| b.second >= now - secs && b.second < now)
            .map(point)
            .collect()
    }

    pub fn point_at(&self, second: i64) -> Option<SeriesPoint> {
        self.buckets.iter().find(|b| b.second == second).map(point)
    }

    /// First second in `[now - from_ago, now)` whose failure rate reached `pct`.
    pub fn first_second_failure_above(&self, now: i64, from_ago: i64, pct: f64) -> Option<i64> {
        self.buckets
            .iter()
            .filter(|b| b.second >= now - from_ago && b.second < now && b.tx > 0)
            .find(|b| b.failed as f64 * 100.0 / b.tx as f64 >= pct)
            .map(|b| b.second)
    }
}

fn point(b: &Bucket) -> SeriesPoint {
    SeriesPoint {
        t: b.second,
        tx: b.tx,
        failed: b.failed,
        avg_cu: if b.cu_n == 0 {
            0.0
        } else {
            b.cu_sum as f64 / b.cu_n as f64
        },
        max_cu: b.cu_max,
    }
}

pub fn mean_std(xs: &[f64]) -> (f64, f64) {
    if xs.is_empty() {
        return (0.0, 0.0);
    }
    let mean = xs.iter().sum::<f64>() / xs.len() as f64;
    let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / xs.len() as f64;
    (mean, var.sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_exclude_current_second_and_respect_ranges() {
        let mut w = Window::default();
        for s in 0..100 {
            for i in 0..10 {
                w.record(s, i != 0, Some(1000), 5000, Some("a"), None);
            }
        }
        w.advance(100);
        let last60 = w.stats(100, 60, 0);
        assert_eq!(last60.seconds, 60);
        assert_eq!(last60.tx, 600);
        assert!((last60.failure_rate() - 10.0).abs() < 1e-9);
        assert_eq!(last60.unique_signers, 1);
        assert_eq!(w.chunk_counts(100, 60, 0, 10, 0), vec![100.0; 6]);
        w.flag(100, 30, ANOMALY_FAILURE);
        assert_eq!(w.stats_masked(100, 60, 0, ANOMALY_FAILURE).tx, 300);
        assert_eq!(w.chunk_counts(100, 60, 0, 10, ANOMALY_FAILURE).len(), 3);
    }

    #[test]
    fn history_is_bounded() {
        let mut w = Window::default();
        w.record(0, true, None, 0, None, None);
        w.advance(5000);
        assert_eq!(w.series(5001, 10_000).len() as i64, HISTORY_SECS);
    }
}
