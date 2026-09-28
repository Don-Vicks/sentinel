//! Events pushed to dashboards over SSE.

use crate::metrics::SeriesPoint;
use crate::model::{AlertExecution, Incident, TxSummary};
use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LiveEvent {
    Transactions {
        program_id: String,
        items: Vec<TxSummary>,
    },
    Metrics {
        program_id: String,
        snapshot: Box<ProgramSnapshot>,
    },
    Incident {
        change: IncidentChange,
        incident: Box<Incident>,
    },
    Alert {
        execution: AlertExecution,
    },
    Stream {
        health: StreamHealth,
    },
}

impl LiveEvent {
    pub fn program_id(&self) -> Option<&str> {
        match self {
            Self::Transactions { program_id, .. } | Self::Metrics { program_id, .. } => {
                Some(program_id)
            }
            Self::Incident { incident, .. } => Some(&incident.program_id),
            Self::Alert { execution } => Some(&execution.program_id),
            Self::Stream { .. } => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Transactions { .. } => "transactions",
            Self::Metrics { .. } => "metrics",
            Self::Incident { .. } => "incident",
            Self::Alert { .. } => "alert",
            Self::Stream { .. } => "stream",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentChange {
    Opened,
    Updated,
    Resolved,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorCount {
    pub key: String,
    pub program_id: String,
    pub program_name: String,
    pub instruction: Option<String>,
    pub error: String,
    pub code: Option<u32>,
    pub count: u64,
    pub share: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProgramSnapshot {
    pub program_id: String,
    pub label: String,
    pub health: &'static str,
    pub at: DateTime<Utc>,
    pub warmup_remaining_secs: i64,
    pub total_tx: u64,
    pub total_failed: u64,
    pub tx_60s: u64,
    pub failed_60s: u64,
    pub failure_rate_60s: f64,
    pub tps_10s: f64,
    pub tps_60s: f64,
    pub avg_cu_60s: f64,
    pub max_cu_60s: u64,
    pub unique_signers_60s: usize,
    pub fees_60s_sol: f64,
    pub baseline_failure_rate: f64,
    pub baseline_tps: f64,
    pub baseline_avg_cu: f64,
    pub top_errors: Vec<ErrorCount>,
    pub open_incidents: usize,
    pub last_tx_at: Option<DateTime<Utc>>,
    pub point: Option<SeriesPoint>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StreamHealth {
    pub connected: bool,
    pub transactions_received: u64,
    pub ingest_tps: f64,
    pub last_slot: u64,
    /// Slots between the chain tip seen on the stream and the newest transaction.
    pub slot_lag: u64,
    pub last_transaction_age_ms: Option<i64>,
    /// Transactions Sentinel missed because it fell behind the Vortex bus.
    pub dropped: u64,
    pub programs_streamed: Vec<String>,
    pub uptime_secs: i64,
}
