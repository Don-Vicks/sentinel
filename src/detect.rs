//! Deterministic, explainable detectors. Each compares a short current window
//! with a trailing baseline that excludes it, and says in plain numbers why it
//! fired. `None` means the condition is not met this tick.

use crate::metrics::{mean_std, Window, ANOMALY_ACTIVITY, ANOMALY_COMPUTE, ANOMALY_FAILURE};
use crate::model::{DetectionConfig, IncidentKind, Severity};

#[derive(Debug, Clone)]
pub struct Detection {
    pub kind: IncidentKind,
    pub severity: Severity,
    pub metric: &'static str,
    pub observed: f64,
    pub baseline: f64,
    pub threshold: f64,
    pub summary: String,
    pub explanation: String,
    /// Unix second the anomaly started, when it can be pinned down.
    pub onset: Option<i64>,
}

fn warmed_up(w: &Window, cfg: &DetectionConfig, now: i64) -> bool {
    w.age(now) >= cfg.warmup_secs as i64
}

fn mins(secs: i64) -> String {
    if secs % 60 == 0 {
        format!("{} min", secs / 60)
    } else {
        format!("{secs}s")
    }
}

pub fn failure_spike(w: &Window, cfg: &DetectionConfig, now: i64) -> Option<Detection> {
    if !cfg.failure_enabled || !warmed_up(w, cfg, now) {
        return None;
    }
    let win = cfg.failure_window_secs as i64;
    let base_span = (cfg.baseline_secs as i64).min(w.age(now));
    let cur = w.stats(now, win, 0);
    let base = w.stats_masked(now, base_span, win, ANOMALY_FAILURE);
    if base.tx < cfg.failure_min_tx as u64 || cur.tx < cfg.failure_min_tx as u64 {
        return None;
    }
    let (rate, base_rate) = (cur.failure_rate(), base.failure_rate());
    let threshold = (base_rate * cfg.failure_multiplier).max(base_rate + cfg.failure_min_delta_pct);
    if rate < threshold {
        return None;
    }
    let severity = match rate {
        r if r >= 50.0 => Severity::Critical,
        r if r >= 25.0 => Severity::High,
        _ => Severity::Medium,
    };
    Some(Detection {
        kind: IncidentKind::FailureSpike,
        severity,
        metric: "failure_rate",
        observed: rate,
        baseline: base_rate,
        threshold,
        summary: format!(
            "{rate:.1}% of transactions failing (normally {base_rate:.1}%)"
        ),
        explanation: format!(
            "Failure rate over the last {win}s is {rate:.1}% ({} of {} tx). Baseline over the \
             previous {} (excluding earlier incidents) is {base_rate:.2}% ({} of {} tx). Threshold is max({}× baseline, baseline \
             + {} pp) = {threshold:.1}%.",
            cur.failed,
            cur.tx,
            mins(base_span - win),
            base.failed,
            base.tx,
            cfg.failure_multiplier,
            cfg.failure_min_delta_pct,
        ),
        onset: w.failure_onset(now, win, threshold, 5),
    })
}

pub fn activity_spike(w: &Window, cfg: &DetectionConfig, now: i64) -> Option<Detection> {
    if !cfg.activity_enabled || !warmed_up(w, cfg, now) {
        return None;
    }
    let win = cfg.activity_window_secs as i64;
    let base_span = (cfg.baseline_secs as i64).min(w.age(now));
    let chunks = w.chunk_counts(now, base_span, win, win, ANOMALY_ACTIVITY);
    if chunks.len() < 5 {
        return None;
    }
    let (mean, std) = mean_std(&chunks);
    let cur = w.stats(now, win, 0).tx as f64;
    let threshold = (mean * cfg.activity_multiplier)
        .max(mean + cfg.activity_z * std)
        .max(cfg.activity_min_tps * win as f64);
    if cur < threshold {
        return None;
    }
    let ratio = if mean > 0.0 { cur / mean } else { f64::INFINITY };
    let severity = match ratio {
        r if r >= 10.0 => Severity::High,
        r if r >= 5.0 => Severity::Medium,
        _ => Severity::Low,
    };
    let (tps, base_tps) = (cur / win as f64, mean / win as f64);
    Some(Detection {
        kind: IncidentKind::ActivitySpike,
        severity,
        metric: "tps",
        observed: tps,
        baseline: base_tps,
        threshold: threshold / win as f64,
        summary: format!("{tps:.1} TPS, {ratio:.1}× the normal {base_tps:.2} TPS"),
        explanation: format!(
            "{cur:.0} transactions in the last {win}s. Over the previous {} the program averaged \
             {mean:.1} ± {std:.1} per {win}s. Threshold is max({}× mean, mean + {}σ, {} TPS floor) \
             = {threshold:.0} per {win}s.",
            mins(base_span - win),
            cfg.activity_multiplier,
            cfg.activity_z,
            cfg.activity_min_tps,
        ),
        onset: Some(now - win),
    })
}

pub fn activity_drop(w: &Window, cfg: &DetectionConfig, now: i64) -> Option<Detection> {
    if !cfg.activity_drop_enabled || !warmed_up(w, cfg, now) {
        return None;
    }
    let quiet = 60;
    let base_span = (cfg.baseline_secs as i64).min(w.age(now));
    if base_span <= quiet {
        return None;
    }
    let base = w.stats_masked(now, base_span, quiet, ANOMALY_ACTIVITY);
    let cur = w.stats(now, quiet, 0);
    // Only meaningful for programs that are normally busy.
    if base.tps() < 0.5 || cur.tx > 0 {
        return None;
    }
    Some(Detection {
        kind: IncidentKind::ActivityDrop,
        severity: Severity::High,
        metric: "tps",
        observed: 0.0,
        baseline: base.tps(),
        threshold: 0.0,
        summary: format!("No transactions for {quiet}s (normally {:.1} TPS)", base.tps()),
        explanation: format!(
            "Zero transactions reached the program in the last {quiet}s, while it averaged {:.2} \
             TPS ({} tx) over the previous {}. Either the program stopped being used or the \
             stream stopped delivering; check stream health.",
            base.tps(),
            base.tx,
            mins(base_span - quiet),
        ),
        onset: Some(now - quiet),
    })
}

pub fn compute_spike(w: &Window, cfg: &DetectionConfig, now: i64) -> Option<Detection> {
    if !cfg.compute_enabled || !warmed_up(w, cfg, now) {
        return None;
    }
    let win = cfg.compute_window_secs as i64;
    let base_span = (cfg.baseline_secs as i64).min(w.age(now));
    let cur = w.stats(now, win, 0);
    let base = w.stats_masked(now, base_span, win, ANOMALY_COMPUTE);
    if cur.cu_n < cfg.compute_min_tx as u64 || base.cu_n < 2 * cfg.compute_min_tx as u64 {
        return None;
    }
    let (avg, base_avg) = (cur.avg_cu(), base.avg_cu());
    let threshold = base_avg * cfg.compute_multiplier;
    if avg < threshold {
        return None;
    }
    let severity = if avg >= base_avg * 4.0 {
        Severity::High
    } else {
        Severity::Medium
    };
    Some(Detection {
        kind: IncidentKind::ComputeSpike,
        severity,
        metric: "avg_compute",
        observed: avg,
        baseline: base_avg,
        threshold,
        summary: format!(
            "Average compute {:.0} CU, {:.1}× the normal {:.0} CU",
            avg,
            avg / base_avg.max(1.0),
            base_avg
        ),
        explanation: format!(
            "Average compute units per transaction over the last {win}s is {avg:.0} (max {}), \
             across {} tx. Baseline over the previous {} is {base_avg:.0}. Threshold is {}× \
             baseline = {threshold:.0} CU.",
            cur.cu_max,
            cur.cu_n,
            mins(base_span - win),
            cfg.compute_multiplier,
        ),
        onset: Some(now - win),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(seconds: i64, per_sec: u32, fail_every: u32) -> Window {
        let mut w = Window::default();
        for s in 0..seconds {
            for i in 0..per_sec {
                let ok = fail_every == 0 || i % fail_every != 0;
                w.record(s, ok, Some(50_000), 5000, Some("x"), None);
            }
        }
        w
    }

    #[test]
    fn failure_spike_fires_only_above_baseline() {
        let cfg = DetectionConfig::default();
        let mut w = filled(600, 5, 0);
        assert!(failure_spike(&w, &cfg, 600).is_none());
        // 60s where 2 of 5 fail each second: 40% vs 0% baseline.
        for s in 600..660 {
            for i in 0..5 {
                w.record(s, i >= 2, Some(50_000), 5000, Some("x"), None);
            }
        }
        let d = failure_spike(&w, &cfg, 660).expect("should fire");
        assert!((d.observed - 40.0).abs() < 1e-9);
        assert_eq!(d.severity, Severity::High);
        assert_eq!(d.onset, Some(600));
    }

    #[test]
    fn respects_warmup() {
        let cfg = DetectionConfig::default();
        let w = filled(60, 5, 1);
        assert!(failure_spike(&w, &cfg, 60).is_none());
    }

    #[test]
    fn activity_spike_and_drop() {
        let cfg = DetectionConfig::default();
        let mut w = filled(600, 2, 0);
        for s in 600..620 {
            for _ in 0..20 {
                w.record(s, true, Some(1), 0, None, None);
            }
        }
        assert!(activity_spike(&w, &cfg, 620).is_some());

        let mut quiet = filled(600, 2, 0);
        quiet.advance(661);
        assert!(activity_drop(&quiet, &cfg, 661).is_some());
    }
}

/// One error type (fingerprint) surging.
pub struct ErrorSpike {
    pub key: String,
    pub new: bool,
    pub detection: Detection,
}

/// Flags error types that surge against their own baseline, or that appear
/// for the first time after warmup. `describe` renders a fingerprint key for
/// humans; `first_seen` is the unix second each fingerprint was first observed.
pub fn error_spikes(
    w: &Window,
    cfg: &DetectionConfig,
    now: i64,
    first_seen: &std::collections::HashMap<String, i64>,
    describe: &dyn Fn(&str) -> String,
) -> Vec<ErrorSpike> {
    if !cfg.error_enabled || !warmed_up(w, cfg, now) {
        return vec![];
    }
    let win = cfg.error_window_secs as i64;
    let base_span = (cfg.baseline_secs as i64).min(w.age(now));
    let cur = w.stats(now, win, 0);
    let base = w.stats_masked(now, base_span, win, ANOMALY_FAILURE);
    let base_windows = (base.seconds as f64 / win as f64).max(1.0);
    let armed_at = w.first_second.unwrap_or(now) + cfg.warmup_secs as i64;

    let mut out = Vec::new();
    for (key, &count) in &cur.fingerprints {
        let count = count as f64;
        let expected = base.fingerprints.get(key).copied().unwrap_or(0) as f64 / base_windows;
        let new = expected == 0.0 && first_seen.get(key).is_some_and(|&f| f >= armed_at);
        let threshold = if new {
            cfg.error_new_min_count as f64
        } else {
            (expected * cfg.error_multiplier)
                .max(expected + 4.0 * expected.sqrt() + 3.0)
                .max(cfg.error_min_count as f64)
        };
        if count < threshold {
            continue;
        }
        let share = if cur.tx > 0 { count / cur.tx as f64 } else { 0.0 };
        let severity = match share {
            s if s >= 0.2 => Severity::High,
            s if s >= 0.05 || new => Severity::Medium,
            _ => Severity::Low,
        };
        let what = describe(key);
        let (summary, explanation) = if new {
            (
                format!("New error: {what} ({count:.0} in {win}s)"),
                format!(
                    "{what} appeared {count:.0} times in the last {win}s ({:.1}% of transactions). It was \
                     never seen during the previous {} of monitoring. Threshold for a new error is {} \
                     occurrences in {win}s.",
                    share * 100.0,
                    mins(now - w.first_second.unwrap_or(now) - win),
                    cfg.error_new_min_count,
                ),
            )
        } else {
            (
                format!("{what}: {count:.0} in {win}s (normally {expected:.1})"),
                format!(
                    "{what} occurred {count:.0} times in the last {win}s ({:.1}% of transactions). Over \
                     the previous {} (excluding earlier incidents) it averaged {expected:.2} per {win}s. \
                     Threshold is max({}× expected, expected + 4√expected + 3, {}) = {threshold:.1}.",
                    share * 100.0,
                    mins(base_span - win),
                    cfg.error_multiplier,
                    cfg.error_min_count,
                ),
            )
        };
        out.push(ErrorSpike {
            key: key.clone(),
            new,
            detection: Detection {
                kind: IncidentKind::ErrorSpike,
                severity,
                metric: "error_count",
                observed: count,
                baseline: expected,
                threshold,
                summary,
                explanation,
                onset: Some(now - win),
            },
        });
    }
    out.sort_by(|a, b| b.detection.observed.total_cmp(&a.detection.observed));
    out
}

#[cfg(test)]
mod error_spike_tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn surging_error_and_new_error() {
        let cfg = DetectionConfig::default();
        let mut w = Window::default();
        let mut first_seen = HashMap::new();
        first_seen.insert("a".to_string(), 0);
        // 10 min: 10 tx/s, error "a" once every 10s.
        for s in 0..600 {
            for i in 0..10 {
                let fp = (s % 10 == 0 && i == 0).then(|| "a".to_string());
                w.record(s, fp.is_none(), None, 0, None, fp);
            }
        }
        assert!(error_spikes(&w, &cfg, 600, &first_seen, &|k| k.to_string()).is_empty());
        // Next 60s: "a" 3 times a second, and a brand-new "b" 10 times.
        first_seen.insert("b".to_string(), 610);
        for s in 600..660 {
            for i in 0..10 {
                let fp = if i < 3 { Some("a") } else if s % 6 == 0 && i == 3 { Some("b") } else { None };
                w.record(s, fp.is_none(), None, 0, None, fp.map(str::to_string));
            }
        }
        let spikes = error_spikes(&w, &cfg, 660, &first_seen, &|k| k.to_string());
        let a = spikes.iter().find(|s| s.key == "a").expect("a surges");
        assert!(!a.new && (a.detection.observed - 180.0).abs() < 1e-9);
        assert_eq!(a.detection.severity, Severity::High);
        let b = spikes.iter().find(|s| s.key == "b").expect("b is new");
        assert!(b.new);
    }
}
