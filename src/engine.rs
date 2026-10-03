//! The Sentinel engine: consumes the Vortex transaction stream, keeps rolling
//! metrics per program, runs detectors and alert rules once a second, and
//! turns what fires into incidents linked to the transactions behind them.

use crate::alerts::{Alert, Dispatcher};
use crate::analyze::{fingerprint_with, summarize, symbol_for};
use crate::idl::IdlRegistry;
use crate::detect::{self, Detection};
use crate::live::{ErrorCount, IncidentChange, InstructionStat, LiveEvent, ProgramSnapshot, StreamHealth};
use crate::metrics::Window;
use crate::model::*;
use crate::source::VortexSource;
use crate::pricing::PriceBook;
use crate::store::Store;
use crate::writer::Writer;
use crate::trace::{program_label, OwnerCache};
use anyhow::{bail, Result};
use chrono::{DateTime, TimeZone, Utc};
use serde_json::json;
use solana_client::nonblocking::rpc_client::RpcClient;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use vortex::events::{TransferKind, VortexTransaction};

const RECENT_SUMMARIES: usize = 500;
const RECENT_FULL: usize = 3_000;
const TX_INDEX_CAP: usize = 10_000;
const MAX_LINKED_PER_INCIDENT: i64 = 300;
const FEED_BATCH: usize = 60;
/// Event-driven incidents (large transfers, transfer rules) close after this
/// long without a new matching transaction.
const EVENT_INCIDENT_QUIET_SECS: i64 = 300;
const ERROR_SPIKE_PREFIX: &str = "error_spike:";
/// Owner of programs configured at startup (`SENTINEL_PROGRAMS`).
pub const SYSTEM: &str = "system";
const MAX_OPEN_ERROR_SPIKES: usize = 3;

pub struct Sentinel {
    pub store: Arc<Store>,
    pub source: Arc<dyn VortexSource>,
    pub rpc: Option<Arc<RpcClient>>,
    pub owners: OwnerCache,
    pub prices: Arc<PriceBook>,
    pub idls: Arc<IdlRegistry>,
    pub beam: crate::beam::BeamClient,
    /// The chain tip according to RPC, and when it was read; the stream's freshness is judged against it.
    chain_tip: Mutex<(u64, i64)>,
    /// Transactions fetched over RPC, so reopening one doesn't hit the network again.
    rpc_txs: Mutex<HashMap<String, Arc<VortexTransaction>>>,
    pub auth: crate::auth::Auth,
    pub limits: crate::limits::Limits,
    pub live: broadcast::Sender<Arc<LiveEvent>>,
    pub public_url: String,
    dispatcher: Dispatcher,
    writer: Writer,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    programs: HashMap<String, ProgramState>,
    rules: Vec<AlertRule>,
    tx_index: HashMap<String, Arc<VortexTransaction>>,
    tx_order: VecDeque<String>,
    dropped: u64,
    newest_tx_slot: u64,
    ingest: Window,
    /// The chain tip as last seen on the feed, and when it last moved. A tip that stops
    /// moving means the feed is dead, not that every program went quiet.
    feed_slot: u64,
    feed_moved_at: i64,
    /// Detectors stay paused until this time after a stall, while the gap washes out of the windows.
    hold_until: i64,
    stalled: bool,
}

/// Seconds without the chain tip advancing before the feed counts as stalled.
const STALL_SECS: i64 = 15;
/// Seconds detectors stay paused after the feed comes back.
const RESUME_GRACE_SECS: i64 = 90;

struct ProgramState {
    program: MonitoredProgram,
    window: Window,
    recent: VecDeque<TxSummary>,
    recent_full: VecDeque<(Arc<VortexTransaction>, TxSummary)>,
    fingerprints: HashMap<String, Fingerprint>,
    fingerprint_first_seen: HashMap<String, i64>,
    open: HashMap<String, OpenIncident>,
    pending_feed: Vec<TxSummary>,
    rule_firing: HashMap<i64, bool>,
    rule_last_fired: HashMap<i64, i64>,
    /// Accounts watching this program ("system" for startup programs).
    watchers: HashSet<String>,
    last_tx_at: Option<DateTime<Utc>>,
}

#[derive(Clone)]
enum LinkFilter {
    Failed,
    ComputeAbove(f64),
    All,
    /// Failed transactions with one specific failure fingerprint.
    Fingerprint(String),
    /// Linked explicitly by whoever raised the incident.
    Manual,
}

impl LinkFilter {
    fn matches(&self, tx: &VortexTransaction, summary: &TxSummary) -> bool {
        match self {
            LinkFilter::Failed => !tx.success,
            LinkFilter::ComputeAbove(t) => summary.compute_units.is_some_and(|cu| cu as f64 >= *t),
            LinkFilter::All => true,
            LinkFilter::Fingerprint(fp) => summary.fingerprint.as_deref() == Some(fp.as_str()),
            LinkFilter::Manual => false,
        }
    }
}

struct OpenIncident {
    incident: Incident,
    link: LinkFilter,
    event_based: bool,
    quiet_since: Option<i64>,
    last_event: i64,
    fingerprint_counts: HashMap<String, (u64, Vec<String>)>,
    wallets: HashSet<String>,
    /// Signatures already linked (bounded by MAX_LINKED_PER_INCIDENT).
    linked: HashSet<String>,
    dirty: bool,
}

impl ProgramState {
    fn new(program: MonitoredProgram) -> Self {
        Self {
            program,
            window: Window::default(),
            recent: VecDeque::new(),
            recent_full: VecDeque::new(),
            fingerprints: HashMap::new(),
            fingerprint_first_seen: HashMap::new(),
            open: HashMap::new(),
            pending_feed: Vec::new(),
            rule_firing: HashMap::new(),
            rule_last_fired: HashMap::new(),
            watchers: HashSet::new(),
            last_tx_at: None,
        }
    }
}

fn ts(second: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(second, 0).single().unwrap_or_else(Utc::now)
}

impl Sentinel {
    pub fn new(
        store: Arc<Store>,
        source: Arc<dyn VortexSource>,
        rpc: Option<Arc<RpcClient>>,
        prices: Arc<PriceBook>,
        public_url: String,
    ) -> Result<Arc<Self>> {
        let (live, _) = broadcast::channel(4096);
        let dispatcher = Dispatcher::new(store.clone(), live.clone());
        let mut state = State {
            rules: store.rules()?,
            ..Default::default()
        };
        for p in store.programs()? {
            state
                .programs
                .insert(p.program_id.clone(), ProgramState::new(p));
        }
        // Detector state is in memory; incidents left open by a previous run
        // can't be tracked to resolution, so close them out.
        for mut inc in store.unresolved_incidents()? {
            inc.status = IncidentStatus::Resolved;
            inc.resolved_at = Some(Utc::now());
            inc.summary = format!("{} (closed on restart)", inc.summary);
            store.update_incident(&inc)?;
        }
        // Who watches what. Programs nobody watches (monitored before
        // accounts existed) belong to "system" so they keep running.
        for (account, program_id) in store.watchlist()? {
            if let Some(ps) = state.programs.get_mut(&program_id) {
                ps.watchers.insert(account);
            }
        }
        for ps in state.programs.values_mut().filter(|ps| ps.watchers.is_empty()) {
            store.watch(SYSTEM, &ps.program.program_id)?;
            ps.watchers.insert(SYSTEM.to_string());
        }
        let idls = IdlRegistry::new(rpc.clone());
        let auth = crate::auth::Auth::new(store.clone(), &public_url);
        let store_for_writer = store.clone();
        let this = Arc::new(Self {
            store,
            source,
            rpc,
            owners: OwnerCache::default(),
            idls,
            beam: crate::beam::BeamClient::new(),
            chain_tip: Mutex::new((0, 0)),
            rpc_txs: Mutex::new(HashMap::new()),
            auth,
            limits: crate::limits::Limits::from_env(),
            prices,
            live,
            public_url,
            dispatcher,
            writer: Writer::spawn(store_for_writer),
            state: Mutex::new(state),
        });
        this.sync_filters();
        Ok(this)
    }

    /// Fetches IDLs for monitored programs; needs a runtime, so it runs from `run`.
    fn request_program_idls(&self) {
        let ids: Vec<String> = self.state.lock().unwrap().programs.keys().cloned().collect();
        for id in ids {
            self.idls.request(&id);
        }
    }

    /// Runs the ingest loop and the 1-second evaluation tick until the source closes.
    pub async fn run(self: Arc<Self>) {
        self.request_program_idls();
        // How far behind the chain is the stream really? Only RPC can say.
        if let Some(rpc) = self.rpc.clone() {
            let this = self.clone();
            tokio::spawn(async move {
                loop {
                    if let Ok(slot) = rpc.get_slot_with_commitment(solana_sdk::commitment_config::CommitmentConfig::processed()).await {
                        *this.chain_tip.lock().unwrap() = (slot, Utc::now().timestamp());
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            });
        }
        let mut rx = self.source.subscribe();
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                msg = rx.recv() => match msg {
                    Ok(tx) => self.on_transaction(tx),
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(dropped = n, "Sentinel fell behind the Vortex bus");
                        self.state.lock().unwrap().dropped += n;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                _ = tick.tick() => self.on_tick(Utc::now().timestamp()),
            }
        }
    }

    fn sync_filters(&self) {
        let ids: Vec<String> = self.state.lock().unwrap().programs.keys().cloned().collect();
        self.source.watch_programs(ids);
    }

    fn emit(&self, event: LiveEvent) {
        let _ = self.live.send(Arc::new(event));
    }

    fn incident_link(&self, id: i64) -> String {
        format!("{}/incidents/{}", self.public_url.trim_end_matches('/'), id)
    }

    // ---------------------------------------------------------------- ingest

    pub fn on_transaction(&self, tx: Arc<VortexTransaction>) {
        let mut guard = self.state.lock().unwrap();
        let state = &mut *guard;
        if state.tx_index.contains_key(&tx.signature) {
            return;
        }
        state.tx_index.insert(tx.signature.clone(), tx.clone());
        state.tx_order.push_back(tx.signature.clone());
        while state.tx_order.len() > TX_INDEX_CAP {
            if let Some(old) = state.tx_order.pop_front() {
                state.tx_index.remove(&old);
            }
        }
        state.newest_tx_slot = state.newest_tx_slot.max(tx.slot);
        let second = tx.received_at.timestamp();
        state.ingest.record(second, true, None, 0, None, None);

        let mut alerts = Vec::new();
        let rules = state.rules.clone();
        let mut noted = false;
        for ps in state.programs.values_mut() {
            let pid = ps.program.program_id.clone();
            if !tx.touches(&pid) {
                continue;
            }
            if !noted {
                for t in &tx.transfers {
                    self.prices.note(t.mint.as_deref());
                }
                noted = true;
            }
            let summary = summarize(&tx, &pid, &self.prices, Some(&self.idls));
            let fp = fingerprint_with(&tx, Some(&self.idls));
            if let Some(fp) = &fp {
                if !ps.fingerprints.contains_key(&fp.key()) {
                    // Name future occurrences from the raising program's IDL.
                    self.idls.request(&fp.program_id);
                }
                ps.fingerprints.entry(fp.key()).or_insert_with(|| fp.clone());
                ps.fingerprint_first_seen.entry(fp.key()).or_insert(second);
            }
            ps.window.record(
                second,
                tx.success,
                summary.compute_units,
                tx.fee,
                tx.fee_payer(),
                fp.as_ref().map(Fingerprint::key),
            );
            let mut names: Vec<String> = summary.instructions.clone();
            names.sort();
            names.dedup();
            if names.is_empty() {
                names.push("(unnamed)".into());
            }
            ps.window.record_instructions(second, &names, tx.success, summary.compute_units);
            ps.last_tx_at = Some(tx.received_at);
            ps.recent.push_front(summary.clone());
            ps.recent.truncate(RECENT_SUMMARIES);
            ps.recent_full.push_front((tx.clone(), summary.clone()));
            ps.recent_full.truncate(RECENT_FULL);
            ps.pending_feed.push(summary.clone());

            let keys: Vec<String> = ps.open.keys().cloned().collect();
            for key in keys {
                self.maybe_link(ps, &key, &tx, &summary, second);
            }

            if ps.program.detection.transfer_enabled {
                if let Some(big) = large_transfer(&tx, &ps.program.detection, &self.prices) {
                    self.large_transfer_incident(ps, &tx, &summary, big, second);
                }
            }
            let applicable: Vec<AlertRule> =
                rules.iter().filter(|r| applies(r, &pid, &ps.watchers)).cloned().collect();
            for rule in &applicable {
                if let Condition::Transfer { mint, min_amount } = &rule.condition {
                    if let Some(t) = tx
                        .transfers
                        .iter()
                        .filter(|t| matches!(t.kind, TransferKind::Sol | TransferKind::Token))
                        .filter(|t| t.mint.as_deref() == mint.as_deref() || (mint.is_none() && t.mint.as_deref() == Some("So11111111111111111111111111111111111111112")))
                        .find(|t| t.amount >= *min_amount)
                    {
                        let msg = format!(
                            "{:.2} {} moved in {} (rule: ≥ {} {})",
                            t.amount,
                            symbol_for(t.mint.as_deref()),
                            short_sig(&tx.signature),
                            min_amount,
                            symbol_for(mint.as_deref())
                        );
                        alerts.extend(self.fire_rule(ps, rule, msg, t.amount, LinkFilter::Manual, true, second, Some((&tx, &summary))));
                    }
                }
                if let Condition::TransferUsd { min_usd } = &rule.condition {
                    let best = tx
                        .transfers
                        .iter()
                        .filter(|t| matches!(t.kind, TransferKind::Sol | TransferKind::Token))
                        .filter_map(|t| {
                            let p = self.prices.get(t.mint.as_deref()).filter(|p| p.trusted())?;
                            Some((t, p.usd * t.amount))
                        })
                        .filter(|(_, usd)| usd >= min_usd)
                        .max_by(|a, b| a.1.total_cmp(&b.1));
                    if let Some((t, usd)) = best {
                        let msg = format!(
                            "{} {} (${}) moved in {} (rule: ≥ ${})",
                            fmt_amount(t.amount),
                            symbol_for(t.mint.as_deref()),
                            fmt_amount(usd),
                            short_sig(&tx.signature),
                            fmt_amount(*min_usd)
                        );
                        alerts.extend(self.fire_rule(ps, rule, msg, usd, LinkFilter::Manual, true, second, Some((&tx, &summary))));
                    }
                }
            }
        }
        drop(guard);
        for a in alerts {
            self.dispatcher.dispatch(a);
        }
    }

    fn maybe_link(
        &self,
        ps: &mut ProgramState,
        key: &str,
        tx: &Arc<VortexTransaction>,
        summary: &TxSummary,
        second: i64,
    ) {
        let Some(open) = ps.open.get_mut(key) else { return };
        if open.link.matches(tx, summary) {
            link_tx(&self.writer, open, tx, summary, second);
        }
    }

    // ------------------------------------------------------------------ tick

    pub fn on_tick(&self, now: i64) {
        let mut alerts = Vec::new();
        let mut guard = self.state.lock().unwrap();
        let state = &mut *guard;
        state.ingest.advance(now);
        let rules = state.rules.clone();

        // Is the feed itself alive? Slots arrive several times a second, so a frozen tip means
        // we are blind. Quiet programs then say nothing about the programs, and the empty
        // seconds must not become part of anyone's baseline.
        let tip = self.source.health().last_slot;
        if tip != state.feed_slot || state.feed_moved_at == 0 {
            state.feed_slot = tip;
            state.feed_moved_at = now;
        }
        let stalled = tip > 0 && now - state.feed_moved_at > STALL_SECS;
        if stalled {
            state.hold_until = now + RESUME_GRACE_SECS;
        }
        if stalled != state.stalled {
            tracing::warn!(stalled, "feed {}", if stalled { "stalled; detectors paused" } else { "recovered" });
        }
        state.stalled = stalled;
        let paused = now < state.hold_until;

        for ps in state.programs.values_mut() {
            ps.window.advance(now);
            if paused {
                use crate::metrics::{ANOMALY_ACTIVITY, ANOMALY_COMPUTE, ANOMALY_FAILURE};
                ps.window.flag(now, 2, ANOMALY_FAILURE | ANOMALY_ACTIVITY | ANOMALY_COMPUTE);
                continue;
            }

            if !ps.pending_feed.is_empty() {
                let mut items = std::mem::take(&mut ps.pending_feed);
                items.reverse();
                items.truncate(FEED_BATCH);
                self.emit(LiveEvent::Transactions {
                    program_id: ps.program.program_id.clone(),
                    items,
                });
            }

            let cfg = ps.program.detection.clone();
            let detections: [(IncidentKind, Option<Detection>); 4] = [
                (IncidentKind::FailureSpike, detect::failure_spike(&ps.window, &cfg, now)),
                (IncidentKind::ActivitySpike, detect::activity_spike(&ps.window, &cfg, now)),
                (IncidentKind::ActivityDrop, detect::activity_drop(&ps.window, &cfg, now)),
                (IncidentKind::ComputeSpike, detect::compute_spike(&ps.window, &cfg, now)),
            ];
            // Keep seconds inside an open incident out of future baselines.
            use crate::metrics::{ANOMALY_ACTIVITY, ANOMALY_COMPUTE, ANOMALY_FAILURE};
            for (kind, bit, win) in [
                (IncidentKind::FailureSpike, ANOMALY_FAILURE, cfg.failure_window_secs),
                (IncidentKind::ActivitySpike, ANOMALY_ACTIVITY, cfg.activity_window_secs),
                (IncidentKind::ActivityDrop, ANOMALY_ACTIVITY, 60),
                (IncidentKind::ComputeSpike, ANOMALY_COMPUTE, cfg.compute_window_secs),
            ] {
                if ps.open.contains_key(kind.as_str()) {
                    ps.window.flag(now, win as i64, bit);
                }
            }
            if ps.open.keys().any(|k| k.starts_with(ERROR_SPIKE_PREFIX)) {
                ps.window.flag(now, cfg.error_window_secs as i64, ANOMALY_FAILURE);
            }
            for (kind, detection) in detections {
                let key = kind.as_str().to_string();
                match detection {
                    Some(d) => {
                        if let Some(opened) = self.on_detection(ps, &key, d, None, now) {
                            alerts.extend(self.incident_rules(&rules, ps, &opened));
                        }
                    }
                    None => self.on_quiet(ps, &key, now, cfg.resolve_after_secs as i64),
                }
            }

            // Error-type spikes. An open failure-spike incident already breaks
            // down the errors it contains, so a surge of one of those isn't
            // opened twice. A brand-new error always gets its own incident.
            let labels = program_labels_one(&ps.program);
            let spikes = {
                let fps = &ps.fingerprints;
                let describe = |key: &str| describe_fingerprint(fps.get(key), key, &labels);
                detect::error_spikes(&ps.window, &cfg, now, &ps.fingerprint_first_seen, &describe)
            };
            let explained: HashSet<String> = ps
                .open
                .get(IncidentKind::FailureSpike.as_str())
                .map(|o| o.fingerprint_counts.keys().cloned().collect())
                .unwrap_or_default();
            let mut firing: HashSet<String> = HashSet::new();
            // Errors raised by other programs inside transactions that merely touch this one
            // (a router's downstream pools, a bot's own program) churn with the market. They
            // count toward the overall failure rate and show in breakdowns, but only errors
            // the monitored program itself raised open an incident.
            let own = format!("{}:", ps.program.program_id);
            for spike in spikes.into_iter().filter(|s| s.key.starts_with(&own)) {
                let key = format!("{ERROR_SPIKE_PREFIX}{}", spike.key);
                let already = ps.open.contains_key(&key);
                let open_count = ps.open.keys().filter(|k| k.starts_with(ERROR_SPIKE_PREFIX)).count();
                let duplicate = !spike.new && explained.contains(&spike.key);
                if !already && (duplicate || open_count >= MAX_OPEN_ERROR_SPIKES) {
                    continue;
                }
                firing.insert(key.clone());
                let link = Some(LinkFilter::Fingerprint(spike.key.clone()));
                if let Some(mut opened) = self.on_detection(ps, &key, spike.detection, link, now) {
                    if spike.new {
                        opened.title = format!("New error · {}", ps.program.label);
                        if let Some(open) = ps.open.get_mut(&key) {
                            open.incident.title = opened.title.clone();
                            open.dirty = true;
                        }
                    }
                    alerts.extend(self.incident_rules(&rules, ps, &opened));
                }
            }
            let quiet: Vec<String> = ps
                .open
                .keys()
                .filter(|k| k.starts_with(ERROR_SPIKE_PREFIX) && !firing.contains(*k))
                .cloned()
                .collect();
            for key in quiet {
                self.on_quiet(ps, &key, now, cfg.resolve_after_secs as i64);
            }

            let pid = ps.program.program_id.clone();
            let applicable: Vec<AlertRule> =
                rules.iter().filter(|r| applies(r, &pid, &ps.watchers)).cloned().collect();
            for rule in &applicable {
                if let Condition::Metric { metric, op, value, window_secs } = &rule.condition {
                    let stats = ps.window.stats(now, *window_secs as i64, 0);
                    let observed = match metric {
                        Metric::FailureRate => stats.failure_rate(),
                        Metric::FailedCount => stats.failed as f64,
                        Metric::Tps => stats.tps(),
                        Metric::TxCount => stats.tx as f64,
                        Metric::AvgCompute => stats.avg_cu(),
                        Metric::MaxCompute => stats.cu_max as f64,
                        Metric::UniqueSigners => stats.unique_signers as f64,
                    };
                    // Rates over an empty window mean nothing.
                    let meaningful = stats.tx > 0 || matches!(metric, Metric::Tps | Metric::TxCount);
                    let hit = meaningful && op.eval(observed, *value);
                    let was = ps.rule_firing.insert(rule.id, hit).unwrap_or(false);
                    let key = format!("rule:{}", rule.id);
                    if hit && !was {
                        let msg = format!(
                            "{} over {}s is {} ({} {})",
                            metric.label(),
                            window_secs,
                            fmt_metric(*metric, observed),
                            op.symbol(),
                            fmt_metric(*metric, *value)
                        );
                        let filter = match metric {
                            Metric::FailureRate | Metric::FailedCount => LinkFilter::Failed,
                            Metric::AvgCompute | Metric::MaxCompute => LinkFilter::ComputeAbove(
                                if *metric == Metric::MaxCompute { *value } else { stats.avg_cu() },
                            ),
                            _ => LinkFilter::All,
                        };
                        alerts.extend(self.fire_rule(ps, rule, msg, observed, filter, false, now, None));
                    } else if hit {
                        if let Some(open) = ps.open.get_mut(&key) {
                            open.quiet_since = None;
                            open.incident.observed = Some(observed);
                            if open.incident.peak.is_none_or(|p| observed > p) {
                                open.incident.peak = Some(observed);
                            }
                        }
                    } else {
                        self.on_quiet(ps, &key, now, cfg.resolve_after_secs as i64);
                    }
                }
            }

            // Event-driven incidents close once things go quiet.
            let stale: Vec<String> = ps
                .open
                .iter()
                .filter(|(_, o)| o.event_based && now - o.last_event >= EVENT_INCIDENT_QUIET_SECS)
                .map(|(k, _)| k.clone())
                .collect();
            for key in stale {
                self.resolve(ps, &key, now);
            }

            // Persist and publish incidents that changed this tick.
            for open in ps.open.values_mut().filter(|o| o.dirty) {
                open.dirty = false;
                refresh_evidence(open, &ps.fingerprints, &program_labels_one(&ps.program));
                open.incident.updated_at = Utc::now();
                let _ = self.store.update_incident(&open.incident);
                self.emit(LiveEvent::Incident {
                    change: IncidentChange::Updated,
                    incident: Box::new(open.incident.clone()),
                });
            }

            let snapshot = snapshot(ps, now);
            self.emit(LiveEvent::Metrics {
                program_id: ps.program.program_id.clone(),
                snapshot: Box::new(snapshot),
            });
        }

        if now % 2 == 0 {
            let health = self.stream_health_locked(state, now);
            self.emit(LiveEvent::Stream { health });
        }
        drop(guard);
        for a in alerts {
            self.dispatcher.dispatch(a);
        }
    }

    /// Opens or refreshes the incident for a firing detector. Returns the
    /// incident when it was newly opened.
    fn on_detection(
        &self,
        ps: &mut ProgramState,
        key: &str,
        d: Detection,
        link: Option<LinkFilter>,
        now: i64,
    ) -> Option<Incident> {
        if let Some(open) = ps.open.get_mut(key) {
            open.quiet_since = None;
            let inc = &mut open.incident;
            inc.observed = Some(d.observed);
            // The headline tracks the worst point; the explanation keeps the
            // numbers from the moment it fired.
            if inc.peak.is_none_or(|p| d.observed > p) {
                inc.peak = Some(d.observed);
                inc.summary = d.summary;
                open.dirty = true;
            }
            if d.severity > inc.severity {
                inc.severity = d.severity;
                open.dirty = true;
            }
            return None;
        }

        let link = link.unwrap_or(match d.kind {
            IncidentKind::FailureSpike => LinkFilter::Failed,
            IncidentKind::ComputeSpike => LinkFilter::ComputeAbove(d.threshold),
            IncidentKind::ActivitySpike => LinkFilter::All,
            _ => LinkFilter::Manual,
        });
        let latest = ps.recent_full.front().map(|(t, _)| t.received_at);
        let incident = Incident {
            id: 0,
            program_id: ps.program.program_id.clone(),
            kind: d.kind,
            severity: d.severity,
            status: IncidentStatus::Open,
            title: format!("{} · {}", d.kind.title(), ps.program.label),
            summary: d.summary,
            explanation: d.explanation,
            source: "detector".into(),
            metric: Some(d.metric.into()),
            observed: Some(d.observed),
            peak: Some(d.observed),
            baseline: Some(d.baseline),
            threshold: Some(d.threshold),
            onset_at: d.onset.map(ts),
            detected_at: Utc::now(),
            updated_at: Utc::now(),
            resolved_at: None,
            detection_latency_ms: latest.map(|l| (Utc::now() - l).num_milliseconds().max(0)),
            affected_count: 0,
            affected_wallets: 0,
            evidence: json!({}),
        };
        let since = d.onset.unwrap_or(now - 60);
        self.open_incident(ps, key, incident, link, false, since, now)
    }

    #[allow(clippy::too_many_arguments)]
    fn open_incident(
        &self,
        ps: &mut ProgramState,
        key: &str,
        incident: Incident,
        link: LinkFilter,
        event_based: bool,
        backfill_since: i64,
        now: i64,
    ) -> Option<Incident> {
        let incident = match self.store.create_incident(incident) {
            Ok(i) => i,
            Err(e) => {
                tracing::error!(error = %e, "failed to create incident");
                return None;
            }
        };
        let mut open = OpenIncident {
            incident,
            link: link.clone(),
            event_based,
            quiet_since: None,
            last_event: now,
            fingerprint_counts: HashMap::new(),
            wallets: HashSet::new(),
            linked: HashSet::new(),
            dirty: false,
        };
        // Link the transactions already seen that belong to this incident.
        let backfill: Vec<_> = ps
            .recent_full
            .iter()
            .filter(|(t, _)| t.received_at.timestamp() >= backfill_since)
            .filter(|(t, s)| link.matches(t, s))
            .cloned()
            .collect();
        for (t, s) in backfill.iter().rev() {
            link_tx(&self.writer, &mut open, t, s, now);
        }
        refresh_evidence(&mut open, &ps.fingerprints, &program_labels_one(&ps.program));
        let _ = self.store.update_incident(&open.incident);
        let opened = open.incident.clone();
        tracing::info!(id = opened.id, kind = ?opened.kind, program = %opened.program_id, "incident opened");
        self.emit(LiveEvent::Incident {
            change: IncidentChange::Opened,
            incident: Box::new(opened.clone()),
        });
        ps.open.insert(key.to_string(), open);
        Some(opened)
    }

    fn on_quiet(&self, ps: &mut ProgramState, key: &str, now: i64, resolve_after: i64) {
        let Some(open) = ps.open.get_mut(key) else { return };
        if open.event_based {
            return;
        }
        let since = *open.quiet_since.get_or_insert(now);
        if now - since >= resolve_after {
            self.resolve(ps, key, now);
        }
    }

    fn resolve(&self, ps: &mut ProgramState, key: &str, now: i64) {
        let Some(mut open) = ps.open.remove(key) else { return };
        refresh_evidence(&mut open, &ps.fingerprints, &program_labels_one(&ps.program));
        // Keep the metric history around the incident once it leaves the
        // in-memory window.
        if let Some(t) = timeline(ps, &open.incident, now) {
            open.incident.evidence["timeline"] = serde_json::to_value(t).unwrap_or_default();
        }
        let inc = &mut open.incident;
        // A human may have marked it resolved already; keep their timestamp.
        if inc.status != IncidentStatus::Resolved {
            inc.status = IncidentStatus::Resolved;
            inc.resolved_at = Some(Utc::now());
        }
        inc.updated_at = Utc::now();
        let _ = self.store.update_incident(inc);
        tracing::info!(id = inc.id, "incident resolved");
        self.emit(LiveEvent::Incident {
            change: IncidentChange::Resolved,
            incident: Box::new(inc.clone()),
        });
    }

    fn large_transfer_incident(
        &self,
        ps: &mut ProgramState,
        tx: &Arc<VortexTransaction>,
        summary: &TxSummary,
        big: LargeMove,
        second: i64,
    ) {
        let key = IncidentKind::LargeTransfer.as_str();
        let value = big.usd.unwrap_or(big.amount);
        if let Some(open) = ps.open.get_mut(key) {
            link_tx(&self.writer, open, tx, summary, second);
            open.last_event = second;
            if open.incident.peak.is_none_or(|p| value > p) {
                open.incident.peak = Some(value);
            }
            return;
        }
        let severity = match big.multiple {
            m if m >= 10.0 => Severity::High,
            m if m >= 3.0 => Severity::Medium,
            _ => Severity::Low,
        };
        let usd_note = big.usd.map(|u| format!(" (${})", fmt_amount(u))).unwrap_or_default();
        let incident = Incident {
            id: 0,
            program_id: ps.program.program_id.clone(),
            kind: IncidentKind::LargeTransfer,
            severity,
            status: IncidentStatus::Open,
            title: format!("Large transfer · {}", ps.program.label),
            summary: format!("{} {}{usd_note} moved in one transaction", fmt_amount(big.amount), big.symbol),
            explanation: format!(
                "Transaction {} moved {:.4} {}{usd_note}, above the {} threshold ({:.1}×). {}\
                 Further large transfers within {} min are grouped here.",
                short_sig(&tx.signature),
                big.amount,
                big.symbol,
                big.threshold_label,
                big.multiple,
                if big.usd_basis {
                    "USD value uses the Solami Blur last-trade price; tokens under $10K liquidity are not valued for alerts. "
                } else {
                    ""
                },
                EVENT_INCIDENT_QUIET_SECS / 60
            ),
            source: "detector".into(),
            metric: Some(if big.usd_basis { "transfer_usd" } else { "transfer_amount" }.into()),
            observed: Some(value),
            peak: Some(value),
            baseline: None,
            threshold: Some(big.threshold),
            onset_at: Some(tx.received_at),
            detected_at: Utc::now(),
            updated_at: Utc::now(),
            resolved_at: None,
            detection_latency_ms: Some((Utc::now() - tx.received_at).num_milliseconds().max(0)),
            affected_count: 0,
            affected_wallets: 0,
            evidence: json!({}),
        };
        if self
            .open_incident(ps, key, incident, LinkFilter::Manual, true, i64::MAX, second)
            .is_some()
        {
            if let Some(open) = ps.open.get_mut(key) {
                link_tx(&self.writer, open, tx, summary, second);
                open.dirty = true;
            }
        }
    }

    /// Fires an alert rule: optionally opens an incident, and returns the
    /// webhook delivery to dispatch once the state lock is released.
    #[allow(clippy::too_many_arguments)]
    fn fire_rule(
        &self,
        ps: &mut ProgramState,
        rule: &AlertRule,
        message: String,
        observed: f64,
        link: LinkFilter,
        event_based: bool,
        now: i64,
        trigger: Option<(&Arc<VortexTransaction>, &TxSummary)>,
    ) -> Option<Alert> {
        let key = format!("rule:{}", rule.id);
        let cooling = ps
            .rule_last_fired
            .get(&rule.id)
            .is_some_and(|t| now - t < rule.cooldown_secs as i64);
        if let Some(open) = ps.open.get_mut(&key) {
            // Already tracking this rule; attach the new evidence and stay quiet.
            if let Some((tx, s)) = trigger {
                link_tx(&self.writer, open, tx, s, now);
                open.last_event = now;
            }
            return None;
        }
        if cooling {
            return None;
        }

        let mut incident_id = None;
        let mut incident_json = serde_json::Value::Null;
        if rule.create_incident {
            let (threshold, window) = match &rule.condition {
                Condition::Metric { value, window_secs, .. } => (Some(*value), *window_secs as i64),
                Condition::Transfer { min_amount, .. } => (Some(*min_amount), 60),
                Condition::TransferUsd { min_usd } => (Some(*min_usd), 60),
                Condition::Incident { .. } => (None, 60),
            };
            let incident = Incident {
                id: 0,
                program_id: ps.program.program_id.clone(),
                kind: IncidentKind::RuleTriggered,
                severity: rule.severity,
                status: IncidentStatus::Open,
                title: format!("{} · {}", rule.name, ps.program.label),
                summary: message.clone(),
                explanation: format!("Alert rule \"{}\" matched: {message}.", rule.name),
                source: format!("rule:{}", rule.id),
                metric: match &rule.condition {
                    Condition::Metric { metric, .. } => serde_json::to_value(metric).ok().and_then(|v| v.as_str().map(String::from)),
                    Condition::TransferUsd { .. } => Some("transfer_usd".to_string()),
                    _ => None,
                },
                observed: Some(observed),
                peak: Some(observed),
                baseline: None,
                threshold,
                onset_at: Some(ts(now - window)),
                detected_at: Utc::now(),
                updated_at: Utc::now(),
                resolved_at: None,
                detection_latency_ms: ps
                    .recent_full
                    .front()
                    .map(|(t, _)| (Utc::now() - t.received_at).num_milliseconds().max(0)),
                affected_count: 0,
                affected_wallets: 0,
                evidence: json!({}),
            };
            if let Some(opened) = self.open_incident(ps, &key, incident, link, event_based, now - window, now) {
                if let (Some((tx, s)), Some(open)) = (trigger, ps.open.get_mut(&key)) {
                    link_tx(&self.writer, open, tx, s, now);
                    open.dirty = true;
                }
                incident_id = Some(opened.id);
                incident_json = serde_json::to_value(&opened).unwrap_or_default();
            }
        }

        ps.rule_last_fired.insert(rule.id, now);
        self.mark_rule_fired(rule.id);
        rule.webhook_url.as_ref()?;
        Some(Alert {
            rule: rule.clone(),
            program_id: ps.program.program_id.clone(),
            program_label: ps.program.label.clone(),
            message: message.clone(),
            incident_id,
            payload: json!({
                "event": "sentinel.alert",
                "rule": { "id": rule.id, "name": rule.name, "condition": rule.condition },
                "severity": rule.severity,
                "program": { "id": ps.program.program_id, "label": ps.program.label },
                "message": message,
                "value": observed,
                "fired_at": Utc::now(),
                "incident": incident_json,
                "transactions": trigger.map(|(t, _)| vec![t.signature.clone()]).unwrap_or_default(),
                "links": { "incident": incident_id.map(|id| self.incident_link(id)) },
            }),
        })
    }

    /// Webhook deliveries for rules that watch for incidents.
    fn incident_rules(&self, rules: &[AlertRule], ps: &ProgramState, incident: &Incident) -> Vec<Alert> {
        let mut out = Vec::new();
        for rule in rules.iter().filter(|r| applies(r, &ps.program.program_id, &ps.watchers)) {
            let Condition::Incident { kinds, min_severity } = &rule.condition else { continue };
            if incident.severity < *min_severity || (!kinds.is_empty() && !kinds.contains(&incident.kind)) {
                continue;
            }
            if rule.webhook_url.is_none() {
                continue;
            }
            self.mark_rule_fired(rule.id);
            out.push(Alert {
                rule: rule.clone(),
                program_id: ps.program.program_id.clone(),
                program_label: ps.program.label.clone(),
                message: format!(
                    "Incident #{} ({:?}): {}",
                    incident.id, incident.severity, incident.summary
                ),
                incident_id: Some(incident.id),
                payload: json!({
                    "event": "sentinel.incident",
                    "rule": { "id": rule.id, "name": rule.name },
                    "severity": incident.severity,
                    "program": { "id": ps.program.program_id, "label": ps.program.label },
                    "message": incident.summary,
                    "incident": incident,
                    "links": { "incident": self.incident_link(incident.id) },
                }),
            });
        }
        out
    }

    /// Records the fire time for display; cooldowns use in-memory state.
    fn mark_rule_fired(&self, rule_id: i64) {
        if let Ok(rules) = self.store.rules() {
            if let Some(mut r) = rules.into_iter().find(|r| r.id == rule_id) {
                r.last_fired_at = Some(Utc::now());
                let _ = self.store.update_rule(&r);
            }
        }
    }

    // ------------------------------------------------------------- queries

    fn stream_health_locked(&self, state: &State, now: i64) -> StreamHealth {
        let hub = self.source.health();
        let last_age = hub
            .last_transaction_at
            .map(|t| (Utc::now() - t).num_milliseconds());
        StreamHealth {
            connected: last_age.is_some_and(|a| a < 15_000),
            transactions_received: hub.transactions,
            ingest_tps: state.ingest.stats(now, 10, 0).tps(),
            last_slot: hub.last_slot,
            slot_lag: hub.last_slot.saturating_sub(state.newest_tx_slot),
            last_transaction_age_ms: last_age,
            dropped: state.dropped,
            programs_streamed: hub.programs,
            uptime_secs: (Utc::now() - hub.started_at).num_seconds(),
            transport: self.source.transport(),
            stalled: state.stalled,
            behind_chain_slots: {
                let (tip, at) = *self.chain_tip.lock().unwrap();
                // A tip older than 30 s, or wildly different (simulation), says nothing.
                let fresh = at > 0 && now - at < 30 && hub.last_slot > 0;
                (fresh && tip.abs_diff(hub.last_slot) < 10_000).then(|| tip.saturating_sub(hub.last_slot))
            },
            pricing: self.prices.status(),
        }
    }

    /// One summary of every Solami product in use, for the dashboard.
    pub fn solami_status(&self) -> crate::live::SolamiStatus {
        use crate::live::*;
        let stream = self.stream_health();
        let pricing = self.prices.status();
        let (lookups, carried) = self.beam.counts();
        SolamiStatus {
            grpc: GrpcStatus {
                connected: stream.connected && !stream.stalled,
                transport: stream.transport,
                tx_per_sec: stream.ingest_tps,
                transactions: stream.transactions_received,
                behind_chain_slots: stream.behind_chain_slots,
            },
            rpc: RpcStatus { enabled: self.rpc.is_some(), idls_loaded: self.idls.loaded() },
            blur: BlurStatus { enabled: pricing.enabled, priced_mints: pricing.priced_mints, error: pricing.last_error },
            mirage: MirageStatus {
                configured: crate::mirage_setup::handle().state() == crate::mirage_setup::State::Ready,
                active: stream.transport == "mirage",
            },
            beam: BeamStatus { lookups, carried },
        }
    }

    pub fn stream_health(&self) -> StreamHealth {
        let state = self.state.lock().unwrap();
        self.stream_health_locked(&state, Utc::now().timestamp())
    }

    /// Waits for queued incident-transaction writes to reach SQLite.
    pub fn flush(&self) {
        self.writer.flush();
    }

    pub fn allow_private_webhooks(&self) -> bool {
        self.dispatcher.allow_private()
    }

    pub fn programs(&self) -> Vec<ProgramSnapshot> {
        let state = self.state.lock().unwrap();
        let now = Utc::now().timestamp();
        let mut out: Vec<_> = state.programs.values().map(|ps| snapshot(ps, now)).collect();
        out.sort_by(|a, b| a.label.cmp(&b.label));
        out
    }

    /// Recent per-second points for every program (overview sparklines).
    pub fn programs_series(&self, secs: i64) -> HashMap<String, Vec<crate::metrics::SeriesPoint>> {
        let state = self.state.lock().unwrap();
        let now = Utc::now().timestamp();
        state
            .programs
            .iter()
            .map(|(id, ps)| (id.clone(), ps.window.series(now, secs)))
            .collect()
    }

    pub fn program_detail(&self, program_id: &str) -> Option<serde_json::Value> {
        let state = self.state.lock().unwrap();
        let ps = state.programs.get(program_id)?;
        let now = Utc::now().timestamp();
        Some(json!({
            "program": ps.program,
            "snapshot": snapshot(ps, now),
            "series": ps.window.series(now, 600),
            "recent": ps.recent.iter().take(100).collect::<Vec<_>>(),
        }))
    }

    /// Metric history around an incident: live from the rolling window when
    /// it still covers the period, otherwise the copy stored at resolution.
    pub fn incident_timeline(&self, incident: &Incident) -> Option<serde_json::Value> {
        let state = self.state.lock().unwrap();
        let live = state
            .programs
            .get(&incident.program_id)
            .and_then(|ps| timeline(ps, incident, Utc::now().timestamp()));
        match live {
            Some(t) => serde_json::to_value(t).ok(),
            None => incident.evidence.get("timeline").cloned(),
        }
    }

    pub fn recent_transactions(&self, program_id: &str, failed_only: bool, limit: usize) -> Vec<TxSummary> {
        let state = self.state.lock().unwrap();
        state
            .programs
            .get(program_id)
            .map(|ps| {
                ps.recent
                    .iter()
                    .filter(|s| !failed_only || !s.success)
                    .take(limit)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Looks in the live window, then incident snapshots, then fetches it
    /// through RPC and the same Vortex decoder.
    pub async fn transaction(&self, signature: &str) -> Result<Option<Arc<VortexTransaction>>> {
        if let Some(tx) = self.state.lock().unwrap().tx_index.get(signature) {
            return Ok(Some(tx.clone()));
        }
        if let Some(tx) = self.store.stored_transaction(signature)? {
            return Ok(Some(Arc::new(tx)));
        }
        if let Some(tx) = self.rpc_txs.lock().unwrap().get(signature) {
            return Ok(Some(tx.clone()));
        }
        let Some(rpc) = &self.rpc else { return Ok(None) };
        // One dropped connection shouldn't fail a page: retry once, and keep what we fetched.
        let mut last = None;
        for attempt in 0..2u64 {
            match vortex::geyser::rpc_frame::fetch_transaction(&rpc.url(), signature).await {
                Ok(found) => {
                    let found = found.map(Arc::new);
                    if let Some(tx) = &found {
                        let mut cache = self.rpc_txs.lock().unwrap();
                        if cache.len() >= 256 {
                            cache.clear();
                        }
                        cache.insert(signature.to_string(), tx.clone());
                    }
                    return Ok(found);
                }
                Err(e) => {
                    last = Some(e);
                    tokio::time::sleep(std::time::Duration::from_millis(400 * (attempt + 1))).await;
                }
            }
        }
        Err(last.expect("two attempts"))
    }

    pub fn program_labels(&self) -> HashMap<String, String> {
        self.state
            .lock()
            .unwrap()
            .programs
            .values()
            .map(|p| (p.program.program_id.clone(), p.program.label.clone()))
            .collect()
    }

    // ------------------------------------------------------------ commands

    /// Monitors a program on behalf of the operator (startup programs, tests).
    pub fn add_program(&self, program_id: String, label: Option<String>) -> Result<MonitoredProgram> {
        self.watch(SYSTEM, program_id, label)
    }

    /// Adds `program_id` to `account`'s watchlist, starting to monitor it if
    /// nobody was yet.
    pub fn watch(&self, account: &str, program_id: String, label: Option<String>) -> Result<MonitoredProgram> {
        let program = self.ensure_program(program_id, label)?;
        self.store.watch(account, &program.program_id)?;
        if let Some(ps) = self.state.lock().unwrap().programs.get_mut(&program.program_id) {
            ps.watchers.insert(account.to_string());
        }
        Ok(program)
    }

    /// Removes a program from `account`'s watchlist; stops monitoring it once
    /// nobody watches it.
    pub fn unwatch(&self, account: &str, program_id: &str) -> Result<()> {
        self.store.unwatch(account, program_id)?;
        let orphaned = {
            let mut state = self.state.lock().unwrap();
            let Some(ps) = state.programs.get_mut(program_id) else { return Ok(()) };
            ps.watchers.remove(account);
            ps.watchers.is_empty()
        };
        if orphaned {
            self.remove_program(program_id)?;
        }
        Ok(())
    }

    pub fn watching(&self, account: &str) -> Vec<String> {
        self.state
            .lock()
            .unwrap()
            .programs
            .values()
            .filter(|ps| ps.watchers.contains(account))
            .map(|ps| ps.program.program_id.clone())
            .collect()
    }

    pub fn is_watching(&self, account: &str, program_id: &str) -> bool {
        self.state
            .lock()
            .unwrap()
            .programs
            .get(program_id)
            .is_some_and(|ps| ps.watchers.contains(account))
    }

    fn ensure_program(&self, program_id: String, label: Option<String>) -> Result<MonitoredProgram> {
        if solana_sdk::pubkey::Pubkey::try_from(program_id.as_str()).is_err() {
            bail!("not a valid base58 public key");
        }
        let label = label
            .filter(|l| !l.trim().is_empty())
            .or_else(|| vortex::events::programs::known_name(&program_id).map(str::to_string))
            .or_else(|| crate::catalog::name_of(&program_id).map(str::to_string))
            .unwrap_or_else(|| crate::analyze::short(&program_id));
        let program = {
            let mut state = self.state.lock().unwrap();
            if let Some(existing) = state.programs.get(&program_id) {
                return Ok(existing.program.clone());
            }
            let program = MonitoredProgram {
                program_id: program_id.clone(),
                label,
                created_at: Utc::now(),
                detection: DetectionConfig::default(),
            };
            self.store.upsert_program(&program)?;
            state
                .programs
                .insert(program_id, ProgramState::new(program.clone()));
            program
        };
        self.sync_filters();
        self.idls.request(&program.program_id);
        Ok(program)
    }

    pub fn update_program(&self, program_id: &str, label: Option<String>, detection: Option<DetectionConfig>) -> Result<MonitoredProgram> {
        let mut state = self.state.lock().unwrap();
        let Some(ps) = state.programs.get_mut(program_id) else { bail!("program not monitored") };
        if let Some(l) = label.filter(|l| !l.trim().is_empty()) {
            ps.program.label = l;
        }
        if let Some(d) = detection {
            ps.program.detection = d;
        }
        self.store.upsert_program(&ps.program)?;
        Ok(ps.program.clone())
    }

    pub fn remove_program(&self, program_id: &str) -> Result<()> {
        self.state.lock().unwrap().programs.remove(program_id);
        self.store.delete_program(program_id)?;
        self.sync_filters();
        Ok(())
    }

    pub fn reload_rules(&self) -> Result<()> {
        let rules = self.store.rules()?;
        let mut state = self.state.lock().unwrap();
        let ids: HashSet<i64> = rules.iter().map(|r| r.id).collect();
        for ps in state.programs.values_mut() {
            ps.rule_firing.retain(|id, _| ids.contains(id));
        }
        state.rules = rules;
        Ok(())
    }

    pub fn set_incident_status(&self, id: i64, status: IncidentStatus) -> Result<Incident> {
        let mut state = self.state.lock().unwrap();
        for ps in state.programs.values_mut() {
            let key = ps.open.iter().find(|(_, o)| o.incident.id == id).map(|(k, _)| k.clone());
            if let Some(key) = key {
                if status == IncidentStatus::Resolved {
                    if let Some(open) = ps.open.get_mut(&key) {
                        open.incident.status = IncidentStatus::Resolved;
                        open.incident.resolved_at = Some(Utc::now());
                    }
                    let inc = ps.open.get(&key).map(|o| o.incident.clone());
                    self.resolve(ps, &key, Utc::now().timestamp());
                    return inc.ok_or_else(|| anyhow::anyhow!("incident vanished"));
                }
                let open = ps.open.get_mut(&key).unwrap();
                open.incident.status = status;
                open.incident.updated_at = Utc::now();
                self.store.update_incident(&open.incident)?;
                self.emit(LiveEvent::Incident {
                    change: IncidentChange::Updated,
                    incident: Box::new(open.incident.clone()),
                });
                return Ok(open.incident.clone());
            }
        }
        drop(state);
        let Some(mut inc) = self.store.incident(id)? else { bail!("incident not found") };
        inc.status = status;
        inc.updated_at = Utc::now();
        if status == IncidentStatus::Resolved && inc.resolved_at.is_none() {
            inc.resolved_at = Some(Utc::now());
        }
        self.store.update_incident(&inc)?;
        self.emit(LiveEvent::Incident {
            change: IncidentChange::Updated,
            incident: Box::new(inc.clone()),
        });
        Ok(inc)
    }

    /// Sends a sample payload through a rule's webhook without waiting for it to fire.
    pub fn test_rule(&self, rule: AlertRule) {
        let program_id = rule.program_id.clone().unwrap_or_else(|| "all".into());
        self.dispatcher.dispatch(Alert {
            message: format!("Test delivery for rule \"{}\"", rule.name),
            payload: json!({
                "event": "sentinel.test",
                "rule": { "id": rule.id, "name": rule.name, "condition": rule.condition },
                "severity": rule.severity,
                "program": { "id": program_id },
                "message": "This is a test delivery from Vortex Sentinel.",
                "fired_at": Utc::now(),
            }),
            program_label: program_id.clone(),
            program_id,
            incident_id: None,
            rule,
        });
    }
}

// ------------------------------------------------------------------ helpers

/// A rule fires for a program when it targets it (or all programs) and its
/// owner watches that program. Ownerless rules predate accounts and apply everywhere.
fn applies(rule: &AlertRule, program_id: &str, watchers: &HashSet<String>) -> bool {
    rule.enabled
        && rule.program_id.as_deref().is_none_or(|p| p == program_id)
        && rule.owner.as_deref().is_none_or(|o| watchers.contains(o))
}

fn link_tx(writer: &Writer, open: &mut OpenIncident, tx: &Arc<VortexTransaction>, summary: &TxSummary, now: i64) {
    // Backfill and live paths can offer the same transaction twice.
    if open.linked.len() < MAX_LINKED_PER_INCIDENT as usize && !open.linked.insert(tx.signature.clone()) {
        return;
    }
    open.last_event = now;
    if let Some(payer) = tx.fee_payer() {
        open.wallets.insert(payer.to_string());
    }
    if let Some(fp) = &summary.fingerprint {
        let entry = open.fingerprint_counts.entry(fp.clone()).or_default();
        entry.0 += 1;
        if entry.1.len() < 5 {
            entry.1.push(tx.signature.clone());
        }
    }
    if open.incident.affected_count < MAX_LINKED_PER_INCIDENT {
        writer.link(open.incident.id, summary.clone(), tx.clone());
    }
    open.incident.affected_count += 1;
    open.incident.affected_wallets = open.wallets.len() as i64;
    open.dirty = true;
}

/// "Pump.fun::Sell → TooLittleSolReceived (#6003)"
fn describe_fingerprint(fp: Option<&Fingerprint>, key: &str, labels: &HashMap<String, String>) -> String {
    let Some(fp) = fp else { return key.to_string() };
    let program = program_label(&fp.program_id, labels);
    let at = match &fp.instruction {
        Some(ix) => format!("{program}::{ix}"),
        None => program,
    };
    match fp.code {
        Some(code) => format!("{} in {at} (#{code})", fp.error),
        None => format!("{} in {at}", fp.error),
    }
}

fn program_labels_one(p: &MonitoredProgram) -> HashMap<String, String> {
    HashMap::from([(p.program_id.clone(), p.label.clone())])
}

fn refresh_evidence(open: &mut OpenIncident, fps: &HashMap<String, Fingerprint>, labels: &HashMap<String, String>) {
    let total: u64 = open.fingerprint_counts.values().map(|(c, _)| c).sum();
    let mut rows: Vec<_> = open.fingerprint_counts.iter().collect();
    rows.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    let fingerprints: Vec<_> = rows
        .into_iter()
        .take(8)
        .map(|(key, (count, samples))| {
            let fp = fps.get(key);
            json!({
                "key": key,
                "program_id": fp.map(|f| f.program_id.clone()),
                "program_name": fp.map(|f| program_label(&f.program_id, labels)),
                "instruction": fp.and_then(|f| f.instruction.clone()),
                "error": fp.map(|f| f.error.clone()),
                "code": fp.and_then(|f| f.code),
                "count": count,
                "share": if total > 0 { *count as f64 / total as f64 } else { 0.0 },
                "samples": samples,
            })
        })
        .collect();
    open.incident.evidence = json!({ "fingerprints": fingerprints, "fingerprinted": total });
}

fn snapshot(ps: &ProgramState, now: i64) -> ProgramSnapshot {
    let cfg = &ps.program.detection;
    let w = &ps.window;
    let s60 = w.stats(now, 60, 0);
    let s10 = w.stats(now, 10, 0);
    let base_span = (cfg.baseline_secs as i64).min(w.age(now));
    // "Normal" as the detectors see it: incident periods excluded.
    use crate::metrics::{ANOMALY_ACTIVITY, ANOMALY_COMPUTE, ANOMALY_FAILURE};
    let base_fail = w.stats_masked(now, base_span, 60, ANOMALY_FAILURE);
    let base_tps = w.stats_masked(now, base_span, 60, ANOMALY_ACTIVITY);
    let base_cu = w.stats_masked(now, base_span, 60, ANOMALY_COMPUTE);
    let labels = program_labels_one(&ps.program);

    let mut errors: Vec<_> = s60.fingerprints.iter().collect();
    errors.sort_by(|a, b| b.1.cmp(a.1));
    // Other programs' errors (bots, a router's pools) are usually louder than the program's
    // own, so keep room for both: otherwise the program's real errors fall off the list.
    let (mut own_n, mut other_n) = (0, 0);
    let top_errors = errors
        .into_iter()
        .filter_map(|(key, count)| {
            let fp = ps.fingerprints.get(key)?;
            let own = fp.program_id == ps.program.program_id;
            let seen = if own { &mut own_n } else { &mut other_n };
            if *seen >= if own { 8 } else { 6 } {
                return None;
            }
            *seen += 1;
            Some(ErrorCount {
                key: key.clone(),
                own,
                program_name: program_label(&fp.program_id, &labels),
                program_id: fp.program_id.clone(),
                instruction: fp.instruction.clone(),
                error: fp.error.clone(),
                code: fp.code,
                count: *count,
                share: if s60.failed > 0 { *count as f64 / s60.failed as f64 } else { 0.0 },
            })
        })
        .collect();

    let s300 = w.stats(now, 300, 0);
    let mut instructions: Vec<InstructionStat> = s300
        .instructions
        .iter()
        .map(|(name, a)| InstructionStat {
            name: name.clone(),
            tx: a.tx,
            failed: a.failed,
            failure_rate: if a.tx > 0 { a.failed as f64 * 100.0 / a.tx as f64 } else { 0.0 },
            avg_cu: if a.cu_n > 0 { a.cu_sum as f64 / a.cu_n as f64 } else { 0.0 },
            share: if s300.tx > 0 { a.tx as f64 / s300.tx as f64 } else { 0.0 },
        })
        .collect();
    instructions.sort_by(|a, b| b.tx.cmp(&a.tx));
    instructions.truncate(12);

    let worst = ps.open.values().map(|o| o.incident.severity).max();
    let warmup_remaining = (cfg.warmup_secs as i64 - w.age(now)).max(0);
    let health = match worst {
        Some(Severity::High | Severity::Critical) => "critical",
        Some(_) => "degraded",
        None if w.first_second.is_none() => "idle",
        None if warmup_remaining > 0 => "warming_up",
        None => "healthy",
    };

    ProgramSnapshot {
        program_id: ps.program.program_id.clone(),
        label: ps.program.label.clone(),
        health,
        at: Utc::now(),
        warmup_remaining_secs: warmup_remaining,
        total_tx: w.total_tx,
        total_failed: w.total_failed,
        tx_60s: s60.tx,
        failed_60s: s60.failed,
        failure_rate_60s: s60.failure_rate(),
        tps_10s: s10.tps(),
        tps_60s: s60.tps(),
        avg_cu_60s: s60.avg_cu(),
        max_cu_60s: s60.cu_max,
        unique_signers_60s: s60.unique_signers,
        fees_60s_sol: s60.fees as f64 / 1e9,
        baseline_failure_rate: base_fail.failure_rate(),
        baseline_tps: base_tps.tps(),
        baseline_avg_cu: base_cu.avg_cu(),
        top_errors,
        instructions,
        open_incidents: ps.open.len(),
        last_tx_at: ps.last_tx_at,
        point: w.point_at(now - 1),
    }
}

/// The most significant transfer over a threshold, by multiple of it.
struct LargeMove {
    amount: f64,
    symbol: String,
    usd: Option<f64>,
    threshold: f64,
    threshold_label: String,
    multiple: f64,
    usd_basis: bool,
}

fn large_transfer(tx: &VortexTransaction, cfg: &DetectionConfig, prices: &PriceBook) -> Option<LargeMove> {
    if !tx.success {
        return None;
    }
    let mut best: Option<LargeMove> = None;
    for t in tx
        .transfers
        .iter()
        .filter(|t| matches!(t.kind, TransferKind::Sol | TransferKind::Token))
    {
        let symbol = symbol_for(t.mint.as_deref());
        let price = prices.get(t.mint.as_deref());
        let usd = price.map(|p| p.usd * t.amount);
        let by_amount = cfg.transfer_thresholds.iter().find(|th| match (&th.mint, &t.mint) {
            (None, None) => true,
            (None, Some(m)) => m == crate::pricing::WSOL,
            (Some(a), Some(b)) => a == b,
            _ => false,
        });
        let mut candidates = Vec::new();
        if let Some(th) = by_amount.filter(|th| t.amount >= th.amount) {
            candidates.push(LargeMove {
                amount: t.amount,
                symbol: symbol.clone(),
                usd,
                threshold: th.amount,
                threshold_label: format!("{} {}", fmt_amount(th.amount), symbol_for(th.mint.as_deref())),
                multiple: t.amount / th.amount,
                usd_basis: false,
            });
        }
        if let (Some(th), Some(p)) = (cfg.transfer_usd_threshold, price.filter(|p| p.trusted())) {
            let value = p.usd * t.amount;
            if value >= th {
                candidates.push(LargeMove {
                    amount: t.amount,
                    symbol: symbol.clone(),
                    usd: Some(value),
                    threshold: th,
                    threshold_label: format!("${}", fmt_amount(th)),
                    multiple: value / th,
                    usd_basis: true,
                });
            }
        }
        for c in candidates {
            if best.as_ref().is_none_or(|b| c.multiple > b.multiple) {
                best = Some(c);
            }
        }
    }
    best
}

fn short_sig(sig: &str) -> String {
    format!("{}…", &sig[..sig.len().min(10)])
}

fn fmt_amount(x: f64) -> String {
    if x >= 1_000_000.0 {
        format!("{:.2}M", x / 1_000_000.0)
    } else if x >= 10_000.0 {
        format!("{:.1}K", x / 1_000.0)
    } else {
        format!("{x:.2}")
    }
}

fn fmt_metric(metric: Metric, v: f64) -> String {
    match metric {
        Metric::FailureRate => format!("{v:.1}%"),
        Metric::Tps => format!("{v:.2}"),
        _ => format!("{v:.0}"),
    }
}

const TIMELINE_BUCKET: i64 = 10;
const TIMELINE_LEAD: i64 = 300;
const TIMELINE_TAIL: i64 = 120;

#[derive(serde::Serialize)]
pub struct Timeline {
    pub bucket_secs: i64,
    pub start: i64,
    pub end: i64,
    pub onset: Option<i64>,
    pub detected: i64,
    pub resolved: Option<i64>,
    pub fingerprints: Vec<TimelineSeries>,
    pub points: Vec<TimelinePoint>,
}

#[derive(serde::Serialize)]
pub struct TimelineSeries {
    pub key: String,
    pub label: String,
}

#[derive(serde::Serialize)]
pub struct TimelinePoint {
    pub t: i64,
    pub tx: u64,
    pub failed: u64,
    pub failure_rate: f64,
    pub tps: f64,
    pub avg_cu: f64,
    /// Occurrences of each series in `fingerprints`, same order.
    pub errors: Vec<u64>,
}

fn timeline(ps: &ProgramState, inc: &Incident, now: i64) -> Option<Timeline> {
    let w = &ps.window;
    let first = w.first_second?;
    let anchor = inc.onset_at.unwrap_or(inc.detected_at).timestamp();
    let history_floor = now - crate::metrics::HISTORY_SECS + 1;
    // Older than the history we keep: nothing live to show (a saved timeline may exist).
    if anchor < history_floor {
        return None;
    }
    // Right after a program starts the incident can begin before the first recorded second;
    // the chart then simply starts where the data does.
    let floor = history_floor.max(first);
    let start = (anchor - TIMELINE_LEAD).max(floor);
    let end = inc
        .resolved_at
        .map(|r| r.timestamp() + TIMELINE_TAIL)
        .unwrap_or(now)
        .min(now);
    if end <= start {
        return None;
    }
    let labels = program_labels_one(&ps.program);
    let fingerprints: Vec<TimelineSeries> = inc.evidence["fingerprints"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f["key"].as_str())
        .take(3)
        .map(|key| TimelineSeries {
            key: key.to_string(),
            label: describe_fingerprint(ps.fingerprints.get(key), key, &labels),
        })
        .collect();
    let start = start - start.rem_euclid(TIMELINE_BUCKET);
    let points = (start..end)
        .step_by(TIMELINE_BUCKET as usize)
        .map(|t| {
            let s = w.stats(now, now - t, (now - t - TIMELINE_BUCKET).max(0));
            TimelinePoint {
                t,
                tx: s.tx,
                failed: s.failed,
                failure_rate: s.failure_rate(),
                tps: s.tx as f64 / TIMELINE_BUCKET as f64,
                avg_cu: s.avg_cu(),
                errors: fingerprints
                    .iter()
                    .map(|f| s.fingerprints.get(&f.key).copied().unwrap_or(0))
                    .collect(),
            }
        })
        .collect();
    Some(Timeline {
        bucket_secs: TIMELINE_BUCKET,
        start,
        end,
        onset: inc.onset_at.map(|t| t.timestamp()).filter(|&t| t >= start),
        detected: inc.detected_at.timestamp(),
        resolved: inc.resolved_at.map(|t| t.timestamp()),
        fingerprints,
        points,
    })
}
