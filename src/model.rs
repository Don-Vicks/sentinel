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
    /// While set and in the future, notifications for this program are held (a maintenance
    /// window). Incidents still open and are recorded.
    #[serde(default)]
    pub muted_until: Option<DateTime<Utc>>,
    #[serde(default)]
    pub mute_reason: Option<String>,
}

impl MonitoredProgram {
    pub fn is_muted(&self, now: DateTime<Utc>) -> bool {
        self.muted_until.is_some_and(|t| t > now)
    }
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
    /// An error must also be at least this percentage of the program's transactions in the
    /// window, so a few stray errors on a busy program aren't an incident.
    pub error_min_share_pct: f64,

    pub compute_enabled: bool,
    pub compute_window_secs: u32,
    pub compute_multiplier: f64,
    pub compute_min_tx: u32,

    pub transfer_enabled: bool,
    pub transfer_thresholds: Vec<TransferThreshold>,
    /// Any single transfer worth at least this many USD (priced by Blur,
    /// liquid tokens only). `None` disables the USD check.
    pub transfer_usd_threshold: Option<f64>,

    /// Upgrades, upgrade-authority changes and closure of the program itself.
    pub authority_enabled: bool,

    /// One wallet suddenly sending most of a program's traffic.
    pub concentration_enabled: bool,
    /// Percent of the last minute's transactions one wallet must account for...
    pub concentration_share_pct: f64,
    /// ...out of at least this many transactions.
    pub concentration_min_tx: u32,

    /// Vault and treasury accounts (token accounts, or SOL accounts) to watch for drains.
    pub vaults: Vec<String>,
    pub drain_enabled: bool,
    /// Net outflow is judged over this window.
    pub drain_window_secs: u32,
    /// A vault losing at least this percent of its balance in the window is an incident...
    pub drain_pct: f64,
    /// ...as is losing this much value in the window, whatever share of the balance it is.
    pub drain_usd: Option<f64>,
    /// Outflows worth less than this (where priced) are ignored, so a dust vault isn't an incident.
    pub drain_min_usd: f64,
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
            warmup_secs: 300,
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
            error_min_share_pct: 1.0,
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
            authority_enabled: true,
            concentration_enabled: true,
            concentration_share_pct: 60.0,
            concentration_min_tx: 100,
            vaults: Vec::new(),
            drain_enabled: true,
            drain_window_secs: 600,
            drain_pct: 20.0,
            drain_usd: Some(100_000.0),
            drain_min_usd: 1_000.0,
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
    /// The program's code or upgrade authority changed.
    AuthorityChange,
    /// A watched vault lost a large part of its balance.
    VaultDrain,
    /// A program this one calls was upgraded or changed hands.
    DependencyChange,
    /// One wallet is sending most of the program's traffic, unlike before.
    BotActivity,
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
            Self::AuthorityChange => "authority_change",
            Self::VaultDrain => "vault_drain",
            Self::DependencyChange => "dependency_change",
            Self::BotActivity => "bot_activity",
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
            Self::AuthorityChange => "Program upgrade or authority change",
            Self::VaultDrain => "Vault outflow",
            Self::DependencyChange => "Dependency changed",
            Self::BotActivity => "Traffic concentrated in one wallet",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Worth knowing, not worth acting on: a Squads vote, a routine event.
    Info,
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
    /// Wallet that owns the rule. Rules apply only to programs it watches.
    #[serde(default)]
    pub owner: Option<String>,
    pub name: String,
    /// `None` applies the rule to every monitored program.
    pub program_id: Option<String>,
    pub condition: Condition,
    pub create_incident: bool,
    pub severity: Severity,
    /// Legacy single-URL target. Kept so stored rules and old API clients keep
    /// working; it is read as one channel when `channels` is empty.
    pub webhook_url: Option<String>,
    /// Where alerts are delivered. Empty falls back to `webhook_url`.
    #[serde(default)]
    pub channels: Vec<Channel>,
    pub enabled: bool,
    pub cooldown_secs: u32,
    pub created_at: DateTime<Utc>,
    pub last_fired_at: Option<DateTime<Utc>>,
}

impl AlertRule {
    /// The channels this rule delivers to, including a legacy `webhook_url`.
    pub fn targets(&self) -> Vec<Channel> {
        if !self.channels.is_empty() {
            return self.channels.clone();
        }
        match self.webhook_url.as_deref().filter(|u| !u.is_empty()) {
            Some(url) => vec![Channel::from_url(url)],
            None => Vec::new(),
        }
    }

    pub fn has_targets(&self) -> bool {
        !self.channels.is_empty() || self.webhook_url.as_deref().is_some_and(|u| !u.is_empty())
    }

    /// A copy that is safe to return from the API: secrets are masked.
    pub fn masked(&self) -> Self {
        let mut r = self.clone();
        r.channels = r.channels.iter().map(Channel::masked).collect();
        if let Some(url) = r.webhook_url.as_deref() {
            if let ChannelKind::Webhook { url: u } | ChannelKind::Slack { url: u } | ChannelKind::Discord { url: u } =
                Channel::from_url(url).masked().kind
            {
                r.webhook_url = Some(u);
            }
        }
        r
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SummaryPeriod {
    /// The last 24 hours, sent every day.
    Daily,
    /// The last 7 days, sent on Mondays.
    Weekly,
}

impl SummaryPeriod {
    pub fn secs(&self) -> i64 {
        match self {
            Self::Daily => 24 * 3600,
            Self::Weekly => 7 * 24 * 3600,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Daily => "Daily summary",
            Self::Weekly => "Weekly summary",
        }
    }
}

/// A recurring report of what a program did, delivered to chat channels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummarySchedule {
    pub id: i64,
    pub owner: String,
    pub program_id: String,
    pub period: SummaryPeriod,
    /// UTC hour (0-23) it is sent at.
    pub hour_utc: u8,
    pub channels: Vec<Channel>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub last_sent_at: Option<DateTime<Utc>>,
}

impl SummarySchedule {
    /// The most recent moment this report was due at or before `now`.
    pub fn due_at(&self, now: DateTime<Utc>) -> DateTime<Utc> {
        use chrono::{Datelike, Duration, TimeZone};
        let today = Utc
            .with_ymd_and_hms(now.year(), now.month(), now.day(), self.hour_utc.min(23) as u32, 0, 0)
            .single()
            .unwrap_or(now);
        let day = if today <= now { today } else { today - Duration::days(1) };
        match self.period {
            SummaryPeriod::Daily => day,
            SummaryPeriod::Weekly => day - Duration::days(day.weekday().num_days_from_monday() as i64),
        }
    }

    pub fn masked(&self) -> Self {
        let mut s = self.clone();
        s.channels = s.channels.iter().map(Channel::masked).collect();
        s
    }
}

/// An API token for agents (the MCP server). Only its hash is stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiToken {
    pub id: i64,
    pub account: String,
    pub name: String,
    /// "read" can look at everything the account can; "write" can also change things.
    pub scope: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

/// Where an alert is delivered.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChannelKind {
    /// Raw JSON POST.
    Webhook { url: String },
    /// Slack incoming webhook (Block Kit message).
    Slack { url: String },
    Discord { url: String },
    /// Telegram bot: messages reply to the incident's first message.
    Telegram { bot_token: String, chat_id: String },
    /// PagerDuty Events API v2: triggers and resolves by incident.
    Pagerduty { routing_key: String },
    /// Slack app bot token (`xoxb-…`): posts to a channel and keeps an
    /// incident's updates in one thread, which an incoming webhook can't do.
    SlackBot { bot_token: String, channel: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Channel {
    #[serde(flatten)]
    pub kind: ChannelKind,
    /// Only alerts at or above this severity go to this channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_severity: Option<Severity>,
}

/// A named channel an account saved once and reuses across rules.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Destination {
    pub id: i64,
    pub owner: String,
    pub name: String,
    #[serde(flatten)]
    pub channel: Channel,
    pub created_at: DateTime<Utc>,
}

impl Destination {
    pub fn masked(&self) -> Self {
        Self { channel: self.channel.masked(), ..self.clone() }
    }
}

/// Prefix of a masked secret returned by the API. A masked value sent back on
/// update means "keep the stored secret".
pub const MASK: char = '•';

fn mask(secret: &str) -> String {
    let tail: String = secret.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    format!("{MASK}{MASK}{MASK}{MASK}{tail}")
}

/// Webhook URLs carry their secret in the path: show scheme and host only.
fn mask_url(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(u) if u.path().len() > 1 || u.query().is_some() => {
            format!("{}://{}/{MASK}{MASK}{MASK}{MASK}", u.scheme(), u.host_str().unwrap_or_default())
        }
        _ => url.to_string(),
    }
}

impl Channel {
    pub fn from_url(url: &str) -> Self {
        let kind = if url.contains("discord.com/api/webhooks") || url.contains("discordapp.com/api/webhooks") {
            ChannelKind::Discord { url: url.into() }
        } else if url.contains("hooks.slack.com") {
            ChannelKind::Slack { url: url.into() }
        } else {
            ChannelKind::Webhook { url: url.into() }
        };
        Self { kind, min_severity: None }
    }

    pub fn label(&self) -> &'static str {
        match self.kind {
            ChannelKind::Webhook { .. } => "webhook",
            ChannelKind::Slack { .. } => "slack",
            ChannelKind::Discord { .. } => "discord",
            ChannelKind::Telegram { .. } => "telegram",
            ChannelKind::Pagerduty { .. } => "pagerduty",
            ChannelKind::SlackBot { .. } => "slack_bot",
        }
    }

    /// Short, secret-free description for the delivery log.
    pub fn display(&self) -> String {
        match &self.kind {
            ChannelKind::Webhook { url } | ChannelKind::Slack { url } | ChannelKind::Discord { url } => mask_url(url),
            ChannelKind::Telegram { chat_id, .. } => format!("telegram chat {chat_id}"),
            ChannelKind::Pagerduty { .. } => "pagerduty".into(),
            ChannelKind::SlackBot { channel, .. } => format!("slack {channel}"),
        }
    }

    pub fn is_masked(&self) -> bool {
        match &self.kind {
            ChannelKind::Webhook { url } | ChannelKind::Slack { url } | ChannelKind::Discord { url } => url.contains(MASK),
            ChannelKind::Telegram { bot_token, .. } => bot_token.starts_with(MASK),
            ChannelKind::Pagerduty { routing_key } => routing_key.starts_with(MASK),
            ChannelKind::SlackBot { bot_token, .. } => bot_token.starts_with(MASK),
        }
    }

    pub fn masked(&self) -> Self {
        let kind = match &self.kind {
            ChannelKind::Webhook { url } => ChannelKind::Webhook { url: mask_url(url) },
            ChannelKind::Slack { url } => ChannelKind::Slack { url: mask_url(url) },
            ChannelKind::Discord { url } => ChannelKind::Discord { url: mask_url(url) },
            ChannelKind::Telegram { bot_token, chat_id } => ChannelKind::Telegram {
                bot_token: mask(bot_token),
                chat_id: chat_id.clone(),
            },
            ChannelKind::Pagerduty { routing_key } => ChannelKind::Pagerduty { routing_key: mask(routing_key) },
            ChannelKind::SlackBot { bot_token, channel } => ChannelKind::SlackBot {
                bot_token: mask(bot_token),
                channel: channel.clone(),
            },
        };
        Self { kind, min_severity: self.min_severity }
    }

    /// Fills a masked secret from the stored channel it replaces.
    pub fn restore_secret(&mut self, old: &Channel) -> bool {
        if !self.is_masked() {
            return true;
        }
        match (&mut self.kind, &old.kind) {
            (ChannelKind::Webhook { url }, ChannelKind::Webhook { url: o })
            | (ChannelKind::Slack { url }, ChannelKind::Slack { url: o })
            | (ChannelKind::Discord { url }, ChannelKind::Discord { url: o }) => *url = o.clone(),
            (ChannelKind::Telegram { bot_token, .. }, ChannelKind::Telegram { bot_token: o, .. }) => *bot_token = o.clone(),
            (ChannelKind::Pagerduty { routing_key }, ChannelKind::Pagerduty { routing_key: o }) => *routing_key = o.clone(),
            (ChannelKind::SlackBot { bot_token, .. }, ChannelKind::SlackBot { bot_token: o, .. }) => *bot_token = o.clone(),
            _ => return false,
        }
        true
    }
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
    /// The program's health score (0-100) drops below `below`.
    Health { below: u8 },
    /// A wallet that appears in the program's transactions (a keeper, a fee payer) holds less
    /// SOL than it should.
    WalletBalance { account: String, below_sol: f64 },
    /// Sentinel itself can't see the chain: its stream stalled, or RPC keeps failing.
    System {
        /// Empty = any.
        #[serde(default)]
        kinds: Vec<SystemKind>,
    },
    /// A call to an instruction, matched on its decoded name, arguments and accounts.
    Instruction {
        /// Instruction names to match, separated by `,` or `|`: `withdraw`, `set_*|update_*`.
        /// Case, underscores and dashes are ignored, so `SetAuthority` matches `set_authority`.
        /// Empty matches every instruction.
        #[serde(default)]
        name: String,
        /// Which program's instruction. Default: the monitored program.
        #[serde(default)]
        program_id: Option<String>,
        /// Conditions on `args.<field>`, `accounts.<name>`, `signer` or `instruction`.
        #[serde(default)]
        filters: Vec<ArgFilter>,
        #[serde(default)]
        match_mode: MatchMode,
        /// Only successful calls (the default). A failed call changed nothing on chain.
        #[serde(default = "yes_default")]
        success_only: bool,
        /// Only when the signer has never called a matching instruction before.
        #[serde(default)]
        first_seen_signer: bool,
    },
    /// A Squads action on a multisig that controls the program: a proposal approved, an
    /// execution, a settings change. Sentinel streams the multisig's transactions for this.
    Squads {
        /// The multisig (v3, v4) or settings account (v5).
        multisig: String,
        /// Squads instruction names to match, as for instructions: `*execute*`, `approve_proposal|proposal_approve`.
        /// Empty matches every action, including ones Sentinel can't name.
        #[serde(default)]
        actions: String,
        /// Only transactions that involve this vault of the multisig (an execution that moves
        /// its funds or signs as it). Proposal votes name no vault, so they never match.
        #[serde(default)]
        vault_index: Option<u8>,
        /// Only successful transactions (the default): a failed action changed nothing.
        #[serde(default = "yes_default")]
        success_only: bool,
    },
    /// An event the program emitted (`emit!` or `emit_cpi!`), matched on its decoded name and fields.
    /// Needs the program's Anchor IDL.
    Event {
        /// Event names to match, as for instructions. Empty matches every event.
        #[serde(default)]
        name: String,
        /// Which program emitted it. Default: the monitored program.
        #[serde(default)]
        program_id: Option<String>,
        /// Conditions on `fields.<name>`, `signer` or `event`.
        #[serde(default)]
        filters: Vec<ArgFilter>,
        #[serde(default)]
        match_mode: MatchMode,
        /// Only events from successful transactions (the default): a failed one never happened.
        #[serde(default = "yes_default")]
        success_only: bool,
    },
}

/// Ways Sentinel can be blind, as opposed to something being wrong with a program.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum SystemKind {
    /// The chain tip stopped advancing on the stream; detectors are paused.
    FeedStalled,
    /// Several RPC calls in a row failed.
    RpcFailing,
}

impl SystemKind {
    pub fn title(&self) -> &'static str {
        match self {
            Self::FeedStalled => "Sentinel's feed has stalled",
            Self::RpcFailing => "Sentinel's RPC is failing",
        }
    }
}

fn yes_default() -> bool {
    true
}

/// One condition on a decoded instruction: `args.amount > 1000000000`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArgFilter {
    pub path: String,
    pub op: FilterOp,
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FilterOp {
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
    /// Text contains the value, or a list holds it.
    Contains,
    /// The field is present and not null; the value is ignored.
    Exists,
}

impl FilterOp {
    pub fn symbol(&self) -> &'static str {
        match self {
            Self::Eq => "=",
            Self::Ne => "≠",
            Self::Gt => ">",
            Self::Gte => "≥",
            Self::Lt => "<",
            Self::Lte => "≤",
            Self::Contains => "contains",
            Self::Exists => "exists",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MatchMode {
    /// Every filter must hold.
    #[default]
    All,
    /// At least one filter holds.
    Any,
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
    #[serde(default)]
    pub owner: Option<String>,
    pub rule_id: i64,
    pub rule_name: String,
    pub program_id: String,
    pub fired_at: DateTime<Utc>,
    pub message: String,
    pub incident_id: Option<i64>,
    /// Secret-free description of the target (kept under its old name).
    pub webhook_url: Option<String>,
    /// `slack`, `telegram`, ... Empty on rows written before channels existed.
    #[serde(default)]
    pub channel: Option<String>,
    /// `opened`, `updated`, `resolved` or `test`.
    #[serde(default)]
    pub event: Option<String>,
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
