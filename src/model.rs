//! Sentinel's own domain types. Transaction data itself is Vortex's
//! `VortexTransaction`; nothing here duplicates it.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitoredProgram {
    pub program_id: String,
    pub label: String,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub detection: DetectionConfig,
}

/// Thresholds for the built-in detectors. Every detector compares a short
/// "current" window against a longer trailing baseline that excludes it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DetectionConfig {
    /// Seconds of history required before any baseline detector may fire.
    pub warmup_secs: u32,
    pub baseline_secs: u32,
    /// A detector must stay quiet this long before its incident auto-resolves.
    pub resolve_after_secs: u32,

    pub failure_enabled: bool,
    pub failure_window_secs: u32,
    pub failure_min_tx: u32,
    pub failure_multiplier: f64,
    /// Percentage points above baseline required, so 0.1% → 0.4% isn't a spike.
    pub failure_min_delta_pct: f64,

    pub activity_enabled: bool,
    pub activity_window_secs: u32,
    pub activity_multiplier: f64,
    pub activity_z: f64,
    pub activity_min_tps: f64,
    pub activity_drop_enabled: bool,

    /// One error type surging, or a new one appearing, even when the
    /// overall failure rate looks normal.
    pub error_enabled: bool,
    pub error_window_secs: u32,
    pub error_min_count: u32,
    pub error_multiplier: f64,
    /// Occurrences within the window for a never-seen error to count.
    pub error_new_min_count: u32,

    pub compute_enabled: bool,
    pub compute_window_secs: u32,
    pub compute_multiplier: f64,
    pub compute_min_tx: u32,

    pub transfer_enabled: bool,
    pub transfer_thresholds: Vec<TransferThreshold>,
    /// Any single transfer worth at least this many USD (priced by Blur,
    /// liquid tokens only). `None` disables the USD check.
    pub transfer_usd_threshold: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferThreshold {
    /// `None` = native SOL.
    pub mint: Option<String>,
    pub amount: f64,
}

impl Default for DetectionConfig {
    fn default() -> Self {
        Self {
            warmup_secs: 120,
            baseline_secs: 600,
            resolve_after_secs: 90,
            failure_enabled: true,
            failure_window_secs: 60,
            failure_min_tx: 20,
            failure_multiplier: 2.5,
            failure_min_delta_pct: 5.0,
            activity_enabled: true,
            activity_window_secs: 20,
            activity_multiplier: 3.0,
            activity_z: 4.0,
            activity_min_tps: 1.0,
            activity_drop_enabled: true,
            error_enabled: true,
            error_window_secs: 60,
            error_min_count: 10,
            error_multiplier: 3.0,
            error_new_min_count: 5,
            compute_enabled: true,
            compute_window_secs: 60,
            compute_multiplier: 2.0,
            compute_min_tx: 10,
            transfer_enabled: true,
            transfer_thresholds: vec![
                TransferThreshold { mint: None, amount: 500.0 },
                TransferThreshold {
                    mint: Some("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".into()),
                    amount: 100_000.0,
                },
                TransferThreshold {
                    mint: Some("Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB".into()),
                    amount: 100_000.0,
                },
            ],
            transfer_usd_threshold: Some(250_000.0),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum IncidentKind {
    FailureSpike,
    ActivitySpike,
    ActivityDrop,
    ComputeSpike,
    LargeTransfer,
    RuleTriggered,
    ErrorSpike,
}

impl IncidentKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::FailureSpike => "failure_spike",
            Self::ActivitySpike => "activity_spike",
            Self::ActivityDrop => "activity_drop",
            Self::ComputeSpike => "compute_spike",
            Self::LargeTransfer => "large_transfer",
            Self::RuleTriggered => "rule_triggered",
            Self::ErrorSpike => "error_spike",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(s.to_string())).ok()
    }

    pub fn title(&self) -> &'static str {
        match self {
            Self::FailureSpike => "Transaction failure spike",
            Self::ActivitySpike => "Activity spike",
            Self::ActivityDrop => "Activity stopped",
            Self::ComputeSpike => "Compute usage spike",
            Self::LargeTransfer => "Large transfer",
            Self::RuleTriggered => "Alert rule triggered",
            Self::ErrorSpike => "Error spike",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IncidentStatus {
    Open,
    Investigating,
    Resolved,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Incident {
    pub id: i64,
    pub program_id: String,
    pub kind: IncidentKind,
    pub severity: Severity,
    pub status: IncidentStatus,
    pub title: String,
    pub summary: String,
    /// Plain-language statement of the rule that fired, with the numbers.
    pub explanation: String,
    pub source: String,
    pub metric: Option<String>,
    pub observed: Option<f64>,
    pub peak: Option<f64>,
    pub baseline: Option<f64>,
    pub threshold: Option<f64>,
    pub onset_at: Option<DateTime<Utc>>,
    pub detected_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    /// Milliseconds between the triggering transaction reaching Sentinel and
    /// the incident opening.
    pub detection_latency_ms: Option<i64>,
    pub affected_count: i64,
    pub affected_wallets: i64,
    pub evidence: serde_json::Value,
}

/// Groups failed transactions by where and why they failed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Fingerprint {
    pub program_id: String,
    pub instruction: Option<String>,
    pub error: String,
    pub code: Option<u32>,
}

impl Fingerprint {
    pub fn key(&self) -> String {
        format!(
            "{}:{}:{}",
            self.program_id,
            self.instruction.as_deref().unwrap_or("?"),
            self.error
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertRule {
    pub id: i64,
    pub name: String,
    /// `None` applies the rule to every monitored program.
    pub program_id: Option<String>,
    pub condition: Condition,
    pub create_incident: bool,
    pub severity: Severity,
    pub webhook_url: Option<String>,
    pub enabled: bool,
    pub cooldown_secs: u32,
    pub created_at: DateTime<Utc>,
    pub last_fired_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Condition {
    Metric {
        metric: Metric,
        op: Op,
        value: f64,
        window_secs: u32,
    },
    Transfer {
        /// `None` = native SOL.
        mint: Option<String>,
        min_amount: f64,
    },
    /// Any single transfer worth at least `min_usd`, priced by Solami Blur.
    TransferUsd {
        min_usd: f64,
    },
    Incident {
        /// Empty = any kind.
        kinds: Vec<IncidentKind>,
        min_severity: Severity,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    FailureRate,
    FailedCount,
    Tps,
    TxCount,
    AvgCompute,
    MaxCompute,
    UniqueSigners,
}

impl Metric {
    pub fn label(&self) -> &'static str {
        match self {
            Self::FailureRate => "failure rate (%)",
            Self::FailedCount => "failed transactions",
            Self::Tps => "TPS",
            Self::TxCount => "transactions",
            Self::AvgCompute => "avg compute units",
            Self::MaxCompute => "max compute units",
            Self::UniqueSigners => "unique signers",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Op {
    #[serde(rename = ">")]
    Gt,
    #[serde(rename = ">=")]
    Gte,
    #[serde(rename = "<")]
    Lt,
    #[serde(rename = "<=")]
    Lte,
}

impl Op {
    pub fn eval(&self, a: f64, b: f64) -> bool {
        match self {
            Self::Gt => a > b,
            Self::Gte => a >= b,
            Self::Lt => a < b,
            Self::Lte => a <= b,
        }
    }

    pub fn symbol(&self) -> &'static str {
        match self {
            Self::Gt => ">",
            Self::Gte => ">=",
            Self::Lt => "<",
            Self::Lte => "<=",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertExecution {
    pub id: i64,
    pub rule_id: i64,
    pub rule_name: String,
    pub program_id: String,
    pub fired_at: DateTime<Utc>,
    pub message: String,
    pub incident_id: Option<i64>,
    pub webhook_url: Option<String>,
    pub delivered: bool,
    pub status_code: Option<u16>,
    pub error: Option<String>,
    pub latency_ms: Option<i64>,
    pub attempts: u32,
}

/// Lightweight row for live feeds and incident transaction lists.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxSummary {
    pub signature: String,
    pub slot: u64,
    pub received_at: DateTime<Utc>,
    pub success: bool,
    pub error: Option<String>,
    pub fingerprint: Option<String>,
    pub compute_units: Option<u64>,
    pub fee: u64,
    pub fee_payer: Option<String>,
    pub instructions: Vec<String>,
    pub transfers: usize,
    pub largest_transfer: Option<LargestTransfer>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LargestTransfer {
    pub amount: f64,
    pub symbol: String,
    pub usd: Option<f64>,
}
