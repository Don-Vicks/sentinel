//! The Sentinel engine: consumes the Vortex transaction stream, keeps rolling
//! metrics per program, runs detectors and alert rules once a second, and
//! turns what fires into incidents linked to the transactions behind them.

use crate::alerts::{Alert, Dispatcher};
use crate::analyze::{fingerprint, summarize, symbol_for};
use crate::detect::{self, Detection};
use crate::live::{ErrorCount, IncidentChange, LiveEvent, ProgramSnapshot, StreamHealth};
use crate::metrics::Window;
use crate::model::*;
use crate::source::VortexSource;
use crate::store::Store;
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

pub struct Sentinel {
    pub store: Arc<Store>,
    pub source: Arc<dyn VortexSource>,
    pub rpc: Option<Arc<RpcClient>>,
    pub owners: OwnerCache,
    pub live: broadcast::Sender<Arc<LiveEvent>>,
    pub public_url: String,
    dispatcher: Dispatcher,
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
}

struct ProgramState {
    program: MonitoredProgram,
    window: Window,
    recent: VecDeque<TxSummary>,
    recent_full: VecDeque<(Arc<VortexTransaction>, TxSummary)>,
    fingerprints: HashMap<String, Fingerprint>,
    open: HashMap<String, OpenIncident>,
    pending_feed: Vec<TxSummary>,
    rule_firing: HashMap<i64, bool>,
    rule_last_fired: HashMap<i64, i64>,
    last_tx_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Copy)]
enum LinkFilter {
    Failed,
    ComputeAbove(f64),
    All,
    /// Linked explicitly by whoever raised the incident.
    Manual,
}

struct OpenIncident {
    incident: Incident,
    link: LinkFilter,
    event_based: bool,
    quiet_since: Option<i64>,
    last_event: i64,
    fingerprint_counts: HashMap<String, (u64, Vec<String>)>,
    wallets: HashSet<String>,
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
            open: HashMap::new(),
            pending_feed: Vec::new(),
            rule_firing: HashMap::new(),
            rule_last_fired: HashMap::new(),
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
        let this = Arc::new(Self {
            store,
            source,
            rpc,
            owners: OwnerCache::default(),
            live,
            public_url,
            dispatcher,
            state: Mutex::new(state),
        });
        this.sync_filters();
        Ok(this)
    }

    /// Runs the ingest loop and the 1-second evaluation tick until the source closes.
    pub async fn run(self: Arc<Self>) {
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
        for ps in state.programs.values_mut() {
            let pid = ps.program.program_id.clone();
            if !tx.touches(&pid) {
                continue;
            }
            let summary = summarize(&tx, &pid);
            let fp = fingerprint(&tx);
            if let Some(fp) = &fp {
                ps.fingerprints.entry(fp.key()).or_insert_with(|| fp.clone());
            }
            ps.window.record(
                second,
                tx.success,
                summary.compute_units,
                tx.fee,
                tx.fee_payer(),
                fp.as_ref().map(Fingerprint::key),
            );
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
                if let Some((amount, symbol, threshold)) = large_transfer(&tx, &ps.program.detection) {
                    self.large_transfer_incident(ps, &tx, &summary, amount, &symbol, threshold, second);
                }
            }
            for rule in rules.iter().filter(|r| applies(r, &pid)) {
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
        let matches = match open.link {
            LinkFilter::Failed => !tx.success,
            LinkFilter::ComputeAbove(t) => summary.compute_units.is_some_and(|cu| cu as f64 >= t),
            LinkFilter::All => true,
            LinkFilter::Manual => false,
        };
        if matches {
            link_tx(&self.store, open, tx, summary, second);
        }
    }

    // ------------------------------------------------------------------ tick

    pub fn on_tick(&self, now: i64) {
        let mut alerts = Vec::new();
        let mut guard = self.state.lock().unwrap();
        let state = &mut *guard;
        state.ingest.advance(now);
        let rules = state.rules.clone();

        for ps in state.programs.values_mut() {
            ps.window.advance(now);

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
            for (kind, detection) in detections {
                let key = kind.as_str().to_string();
                match detection {
                    Some(d) => {
                        if let Some(opened) = self.on_detection(ps, &key, d, now) {
                            alerts.extend(self.incident_rules(&rules, ps, &opened));
                        }
                    }
                    None => self.on_quiet(ps, &key, now, cfg.resolve_after_secs as i64),
                }
            }

            let pid = ps.program.program_id.clone();
            for rule in rules.iter().filter(|r| applies(r, &pid)) {
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
    fn on_detection(&self, ps: &mut ProgramState, key: &str, d: Detection, now: i64) -> Option<Incident> {
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

        let link = match d.kind {
            IncidentKind::FailureSpike => LinkFilter::Failed,
            IncidentKind::ComputeSpike => LinkFilter::ComputeAbove(d.threshold),
            IncidentKind::ActivitySpike => LinkFilter::All,
            _ => LinkFilter::Manual,
        };
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
            link,
            event_based,
            quiet_since: None,
            last_event: now,
            fingerprint_counts: HashMap::new(),
            wallets: HashSet::new(),
            dirty: false,
        };
        // Link the transactions already seen that belong to this incident.
        let backfill: Vec<_> = ps
            .recent_full
            .iter()
            .filter(|(t, _)| t.received_at.timestamp() >= backfill_since)
            .filter(|(t, s)| match link {
                LinkFilter::Failed => !t.success,
                LinkFilter::ComputeAbove(th) => s.compute_units.is_some_and(|cu| cu as f64 >= th),
                LinkFilter::All => true,
                LinkFilter::Manual => false,
            })
            .cloned()
            .collect();
        for (t, s) in backfill.iter().rev() {
            link_tx(&self.store, &mut open, t, s, now);
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

    fn resolve(&self, ps: &mut ProgramState, key: &str, _now: i64) {
        let Some(mut open) = ps.open.remove(key) else { return };
        refresh_evidence(&mut open, &ps.fingerprints, &program_labels_one(&ps.program));
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

    #[allow(clippy::too_many_arguments)]
    fn large_transfer_incident(
        &self,
        ps: &mut ProgramState,
        tx: &Arc<VortexTransaction>,
        summary: &TxSummary,
        amount: f64,
        symbol: &str,
        threshold: f64,
        second: i64,
    ) {
        let key = IncidentKind::LargeTransfer.as_str();
        if let Some(open) = ps.open.get_mut(key) {
            link_tx(&self.store, open, tx, summary, second);
            open.last_event = second;
            if open.incident.peak.is_none_or(|p| amount > p) {
                open.incident.peak = Some(amount);
            }
            return;
        }
        let multiple = amount / threshold;
        let severity = match multiple {
            m if m >= 10.0 => Severity::High,
            m if m >= 3.0 => Severity::Medium,
            _ => Severity::Low,
        };
        let incident = Incident {
            id: 0,
            program_id: ps.program.program_id.clone(),
            kind: IncidentKind::LargeTransfer,
            severity,
            status: IncidentStatus::Open,
            title: format!("Large transfer · {}", ps.program.label),
            summary: format!("{} {symbol} moved in one transaction", fmt_amount(amount)),
            explanation: format!(
                "Transaction {} moved {:.4} {symbol}, above the {} {symbol} threshold ({multiple:.1}×). \
                 Further large transfers within {} min are grouped here.",
                short_sig(&tx.signature),
                amount,
                fmt_amount(threshold),
                EVENT_INCIDENT_QUIET_SECS / 60
            ),
            source: "detector".into(),
            metric: Some("transfer_amount".into()),
            observed: Some(amount),
            peak: Some(amount),
            baseline: None,
            threshold: Some(threshold),
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
                link_tx(&self.store, open, tx, summary, second);
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
                link_tx(&self.store, open, tx, s, now);
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
                metric: None,
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
                    link_tx(&self.store, open, tx, s, now);
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
        for rule in rules.iter().filter(|r| applies(r, &ps.program.program_id)) {
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
        }
    }

    pub fn stream_health(&self) -> StreamHealth {
        let state = self.state.lock().unwrap();
        self.stream_health_locked(&state, Utc::now().timestamp())
    }

    pub fn programs(&self) -> Vec<ProgramSnapshot> {
        let state = self.state.lock().unwrap();
        let now = Utc::now().timestamp();
        let mut out: Vec<_> = state.programs.values().map(|ps| snapshot(ps, now)).collect();
        out.sort_by(|a, b| a.label.cmp(&b.label));
        out
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
        let Some(rpc) = &self.rpc else { return Ok(None) };
        Ok(vortex::geyser::rpc_frame::fetch_transaction(&rpc.url(), signature)
            .await?
            .map(Arc::new))
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

    pub fn add_program(&self, program_id: String, label: Option<String>) -> Result<MonitoredProgram> {
        if solana_sdk::pubkey::Pubkey::try_from(program_id.as_str()).is_err() {
            bail!("not a valid base58 public key");
        }
        let label = label
            .filter(|l| !l.trim().is_empty())
            .or_else(|| vortex::events::programs::known_name(&program_id).map(str::to_string))
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

fn applies(rule: &AlertRule, program_id: &str) -> bool {
    rule.enabled && rule.program_id.as_deref().is_none_or(|p| p == program_id)
}

fn link_tx(store: &Store, open: &mut OpenIncident, tx: &Arc<VortexTransaction>, summary: &TxSummary, now: i64) {
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
    let stored = open.incident.affected_count < MAX_LINKED_PER_INCIDENT
        && store
            .link_transaction(open.incident.id, summary, tx)
            .unwrap_or(false);
    if stored || open.incident.affected_count >= MAX_LINKED_PER_INCIDENT {
        open.incident.affected_count += 1;
    }
    open.incident.affected_wallets = open.wallets.len() as i64;
    open.dirty = true;
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
    let base = w.stats(now, base_span, 60);
    let labels = program_labels_one(&ps.program);

    let mut errors: Vec<_> = s60.fingerprints.iter().collect();
    errors.sort_by(|a, b| b.1.cmp(a.1));
    let top_errors = errors
        .into_iter()
        .take(5)
        .filter_map(|(key, count)| {
            let fp = ps.fingerprints.get(key)?;
            Some(ErrorCount {
                key: key.clone(),
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
        baseline_failure_rate: base.failure_rate(),
        baseline_tps: base.tps(),
        baseline_avg_cu: base.avg_cu(),
        top_errors,
        open_incidents: ps.open.len(),
        last_tx_at: ps.last_tx_at,
        point: w.point_at(now - 1),
    }
}

fn large_transfer(tx: &VortexTransaction, cfg: &DetectionConfig) -> Option<(f64, String, f64)> {
    if !tx.success {
        return None;
    }
    tx.transfers
        .iter()
        .filter(|t| matches!(t.kind, TransferKind::Sol | TransferKind::Token))
        .filter_map(|t| {
            let th = cfg.transfer_thresholds.iter().find(|th| match (&th.mint, &t.mint) {
                (None, None) => true,
                (None, Some(m)) => m == "So11111111111111111111111111111111111111112",
                (Some(a), Some(b)) => a == b,
                _ => false,
            })?;
            (t.amount >= th.amount).then(|| (t.amount, symbol_for(th.mint.as_deref()), th.amount))
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
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
