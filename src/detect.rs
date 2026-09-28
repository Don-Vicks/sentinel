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
        onset: w.first_second_failure_above(now, win, threshold),
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
