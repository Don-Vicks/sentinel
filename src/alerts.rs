//! Alert delivery. A rule can target several channels (Slack, Discord,
//! Telegram, PagerDuty, a plain webhook). Alerts follow the incident: the first
//! message opens it, an escalation and the resolution reply to it, and
//! PagerDuty triggers and resolves under one dedup key.

use crate::live::LiveEvent;
use crate::model::{AlertExecution, AlertRule, Channel, ChannelKind, Severity};
use crate::store::Store;
use chrono::Utc;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, watch};

const ATTEMPTS: u32 = 3;
const TELEGRAM_API: &str = "https://api.telegram.org";
const PAGERDUTY_API: &str = "https://events.pagerduty.com/v2/enqueue";

/// Where in an incident's life an alert sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertEvent {
    Opened,
    /// Escalated to a higher severity.
    Updated,
    Resolved,
    Test,
    /// A scheduled report of what the program did, not an alert.
    Summary,
}

impl AlertEvent {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Opened => "opened",
            Self::Updated => "updated",
            Self::Resolved => "resolved",
            Self::Test => "test",
            Self::Summary => "summary",
        }
    }
}

#[derive(Clone)]
pub struct Alert {
    pub rule: AlertRule,
    pub program_id: String,
    pub program_label: String,
    pub message: String,
    pub incident_id: Option<i64>,
    pub severity: Severity,
    pub event: AlertEvent,
    pub payload: Value,
}

#[derive(Clone)]
pub struct Dispatcher {
    client: reqwest::Client,
    store: Arc<Store>,
    live: broadcast::Sender<Arc<LiveEvent>>,
    allow_private: bool,
    telegram_api: String,
    pagerduty_api: String,
    /// The last delivery queued for each (rule, incident, channel). The next one
    /// waits for it, so an escalation never overtakes the message it replies to.
    chains: Arc<Mutex<HashMap<(i64, i64, usize), watch::Receiver<bool>>>>,
    /// Deliveries by (channel, event, outcome), for /metrics.
    stats: Arc<Mutex<HashMap<(String, String, &'static str), u64>>>,
}

fn is_public(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || o[0] == 0
                || (o[0] == 100 && (64..128).contains(&o[1]))) // carrier-grade NAT
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let seg = v6.segments()[0];
            !(v6.is_loopback()
                || v6.is_unspecified()
                || (seg & 0xfe00) == 0xfc00 // unique local
                || (seg & 0xffc0) == 0xfe80) // link local
        }
    }
}

/// Webhooks go to the public internet only, so a rule can't be used to reach
/// services on the host's private network. `allow_private` is for local dev.
pub async fn check_webhook_url(url: &str, allow_private: bool) -> anyhow::Result<()> {
    let parsed = reqwest::Url::parse(url).map_err(|_| anyhow::anyhow!("Webhook URL is not a valid URL"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        anyhow::bail!("Webhook URL must start with http:// or https://");
    }
    let host = parsed.host_str().ok_or_else(|| anyhow::anyhow!("Webhook URL has no host"))?;
    if allow_private {
        return Ok(());
    }
    let port = parsed.port_or_known_default().unwrap_or(443);
    let addrs: Vec<_> = tokio::net::lookup_host((host.trim_matches(['[', ']']), port))
        .await
        .map_err(|_| anyhow::anyhow!("Webhook host {host} doesn't resolve"))?
        .collect();
    if addrs.is_empty() || addrs.iter().any(|a| !is_public(a.ip())) {
        anyhow::bail!("Webhook host {host} is on a private network; use a public URL");
    }
    Ok(())
}

/// Checks everything about a channel that can be checked without sending.
pub async fn check_channel(channel: &Channel, allow_private: bool) -> anyhow::Result<()> {
    match &channel.kind {
        ChannelKind::Webhook { url } | ChannelKind::Slack { url } | ChannelKind::Discord { url } => {
            check_webhook_url(url, allow_private).await
        }
        ChannelKind::Telegram { bot_token, chat_id } => {
            let (id, secret) = bot_token.split_once(':').unwrap_or(("", ""));
            if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) || secret.len() < 10 {
                anyhow::bail!("Telegram bot token should look like 123456:ABC-DEF... (from @BotFather)");
            }
            if !bot_token.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'_' | b'-')) {
                anyhow::bail!("Telegram bot token has unexpected characters");
            }
            let chat = chat_id.trim();
            let numeric = chat.strip_prefix('-').unwrap_or(chat);
            let handle = chat.strip_prefix('@').unwrap_or("");
            if !(numeric.len() > 0 && numeric.bytes().all(|b| b.is_ascii_digit()))
                && !(handle.len() >= 5 && handle.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
            {
                anyhow::bail!("Telegram chat id should be a number like -1001234567890, or @channelname");
            }
            Ok(())
        }
        ChannelKind::Pagerduty { routing_key } => {
            if routing_key.len() < 20 || !routing_key.bytes().all(|b| b.is_ascii_alphanumeric()) {
                anyhow::bail!("PagerDuty routing key is the 32-character Integration Key of an Events API v2 integration");
            }
            Ok(())
        }
    }
}

struct Request {
    url: String,
    body: Value,
}

struct Outcome {
    status: Option<u16>,
    error: Option<String>,
    body: Option<Value>,
    attempts: u32,
    latency_ms: Option<i64>,
    ok: bool,
}

impl Dispatcher {
    pub fn allow_private(&self) -> bool {
        self.allow_private
    }

    /// Deliveries so far, by channel, event and whether they arrived.
    pub fn delivery_stats(&self) -> Vec<((String, String, &'static str), u64)> {
        let mut v: Vec<_> = self.stats.lock().unwrap().iter().map(|(k, n)| (k.clone(), *n)).collect();
        v.sort();
        v
    }

    pub fn new(store: Arc<Store>, live: broadcast::Sender<Arc<LiveEvent>>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .user_agent("vortex-sentinel/0.1")
            .build()
            .expect("reqwest client");
        let allow_private = std::env::var("SENTINEL_ALLOW_PRIVATE_WEBHOOKS").is_ok_and(|v| v == "1" || v == "true");
        let env_or = |name: &str, default: &str| {
            std::env::var(name)
                .ok()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| default.to_string())
        };
        Self {
            client,
            store,
            live,
            allow_private,
            telegram_api: env_or("SENTINEL_TELEGRAM_API", TELEGRAM_API).trim_end_matches('/').to_string(),
            pagerduty_api: env_or("SENTINEL_PAGERDUTY_API", PAGERDUTY_API),
            chains: Arc::default(),
            stats: Arc::default(),
        }
    }

    /// Records the alert and delivers it to each of the rule's channels in the background.
    pub fn dispatch(&self, alert: Alert) {
        for (idx, channel) in alert.rule.targets().into_iter().enumerate() {
            if !matches!(alert.event, AlertEvent::Test | AlertEvent::Summary) && channel.min_severity.is_some_and(|min| alert.severity < min) {
                continue;
            }
            // A report isn't something to page anyone about.
            if alert.event == AlertEvent::Summary && matches!(channel.kind, ChannelKind::Pagerduty { .. }) {
                continue;
            }
            let this = self.clone();
            let alert = alert.clone();
            let key = alert.incident_id.map(|id| (alert.rule.id, id, idx));
            let (done, rx) = watch::channel(false);
            let previous = key.and_then(|k| {
                let mut chains = self.chains.lock().unwrap();
                if chains.len() > 10_000 {
                    chains.clear();
                }
                // A resolved incident has no later events to order.
                if alert.event == AlertEvent::Resolved {
                    chains.remove(&k)
                } else {
                    chains.insert(k, rx)
                }
            });
            tokio::spawn(async move {
                if let Some(mut previous) = previous {
                    let _ = previous.wait_for(|finished| *finished).await;
                }
                let exec = this.deliver(&alert, idx, &channel).await;
                *this
                    .stats
                    .lock()
                    .unwrap()
                    .entry((channel.label().to_string(), alert.event.as_str().to_string(), if exec.delivered { "delivered" } else { "failed" }))
                    .or_default() += 1;
                let _ = done.send(true);
                match this.store.record_execution(exec) {
                    Ok(exec) => {
                        let _ = this.live.send(Arc::new(LiveEvent::Alert { execution: exec }));
                    }
                    Err(e) => tracing::error!(error = %e, "failed to record alert execution"),
                }
            });
        }
    }

    async fn deliver(&self, alert: &Alert, idx: usize, channel: &Channel) -> AlertExecution {
        let mut exec = AlertExecution {
            id: 0,
            owner: alert.rule.owner.clone(),
            rule_id: alert.rule.id,
            rule_name: alert.rule.name.clone(),
            program_id: alert.program_id.clone(),
            fired_at: Utc::now(),
            message: alert.message.clone(),
            incident_id: alert.incident_id,
            webhook_url: Some(channel.display()),
            channel: Some(channel.label().to_string()),
            event: Some(alert.event.as_str().to_string()),
            delivered: false,
            status_code: None,
            error: None,
            latency_ms: None,
            attempts: 0,
        };
        if let Err(e) = check_channel(channel, self.allow_private).await {
            exec.error = Some(crate::redact::scrub(&e.to_string()));
            return exec;
        }
        let prior = alert
            .incident_id
            .and_then(|id| self.store.delivery_ref(alert.rule.id, id, idx).ok().flatten());
        let delivery_id = uuid::Uuid::new_v4().to_string();
        let requests = self.requests(channel, alert, prior.as_deref(), &delivery_id);

        let mut first_body = None;
        for (n, req) in requests.iter().enumerate() {
            let out = self.post(req, &delivery_id).await;
            exec.attempts = out.attempts;
            exec.status_code = out.status;
            exec.latency_ms = out.latency_ms;
            exec.error = out.error;
            exec.delivered = out.ok;
            if n == 0 {
                first_body = out.body;
            }
            if !out.ok {
                return exec;
            }
        }

        // Remember the message the incident opened with, so later events reply to it.
        if prior.is_none() {
            if let (Some(id), Some(msg_ref)) = (alert.incident_id, first_body.as_ref().and_then(|b| message_ref(channel, b))) {
                if let Err(e) = self.store.set_delivery_ref(alert.rule.id, id, idx, &msg_ref) {
                    tracing::warn!(error = %e, "failed to remember alert message");
                }
            }
        }
        exec
    }

    /// POSTs with retries: transient failures back off, client errors stop (except rate limits).
    async fn post(&self, req: &Request, delivery_id: &str) -> Outcome {
        let mut out = Outcome {
            status: None,
            error: None,
            body: None,
            attempts: 0,
            latency_ms: None,
            ok: false,
        };
        for attempt in 1..=ATTEMPTS {
            out.attempts = attempt;
            let started = Instant::now();
            let res = self
                .client
                .post(&req.url)
                .header("X-Sentinel-Delivery", delivery_id)
                .header("X-Sentinel-Event", "alert")
                .json(&req.body)
                .send()
                .await;
            out.latency_ms = Some(started.elapsed().as_millis() as i64);
            match res {
                Ok(resp) => {
                    let status = resp.status();
                    out.status = Some(status.as_u16());
                    let text = resp.text().await.unwrap_or_default();
                    out.body = serde_json::from_str(&text).ok();
                    if status.is_success() {
                        out.ok = true;
                        out.error = None;
                        return out;
                    }
                    out.error = Some(match api_error(&out.body) {
                        Some(detail) => format!("HTTP {status}: {detail}"),
                        None => format!("HTTP {status}"),
                    });
                    if status.is_client_error() && status.as_u16() != 429 {
                        return out;
                    }
                }
                // The URL holds the bot token or webhook secret; keep it out of the error.
                Err(e) => out.error = Some(crate::redact::scrub(&e.without_url().to_string())),
            }
            tokio::time::sleep(Duration::from_millis(500 * 3u64.pow(attempt - 1))).await;
        }
        out
    }

    fn requests(&self, channel: &Channel, alert: &Alert, prior: Option<&str>, delivery_id: &str) -> Vec<Request> {
        match &channel.kind {
            ChannelKind::Webhook { url } => {
                let mut body = alert.payload.clone();
                if let Some(obj) = body.as_object_mut() {
                    obj.insert("lifecycle".into(), json!(alert.event.as_str()));
                }
                vec![Request { url: url.clone(), body }]
            }
            ChannelKind::Slack { url } => vec![Request {
                url: url.clone(),
                body: slack_body(alert),
            }],
            ChannelKind::Discord { url } => vec![Request {
                url: url.clone(),
                body: discord_body(alert),
            }],
            ChannelKind::Telegram { bot_token, chat_id } => vec![Request {
                url: format!("{}/bot{bot_token}/sendMessage", self.telegram_api),
                body: telegram_body(alert, chat_id.trim(), prior),
            }],
            ChannelKind::Pagerduty { routing_key } => {
                let dedup = match (alert.event, alert.incident_id) {
                    (AlertEvent::Test, _) => format!("sentinel-test-{delivery_id}"),
                    (_, Some(id)) => format!("sentinel-incident-{id}"),
                    _ => format!("sentinel-rule-{}", alert.rule.id),
                };
                let event = |action: &str| Request {
                    url: self.pagerduty_api.clone(),
                    body: pagerduty_body(alert, routing_key, &dedup, action),
                };
                match alert.event {
                    AlertEvent::Resolved => vec![event("resolve")],
                    // A test must not leave an open page behind.
                    AlertEvent::Test => vec![event("trigger"), event("resolve")],
                    _ => vec![event("trigger")],
                }
            }
        }
    }
}

/// The id a channel gives the message it created, used to reply to it later.
fn message_ref(channel: &Channel, body: &Value) -> Option<String> {
    match channel.kind {
        ChannelKind::Telegram { .. } => body["result"]["message_id"].as_i64().map(|id| id.to_string()),
        _ => None,
    }
}

fn api_error(body: &Option<Value>) -> Option<String> {
    let b = body.as_ref()?;
    let text = b["description"].as_str().or_else(|| b["message"].as_str()).or_else(|| b["error"].as_str())?;
    Some(crate::redact::scrub(text).chars().take(160).collect())
}

// ---------------------------------------------------------------- formatting

fn incident_link(alert: &Alert) -> &str {
    alert.payload["links"]["incident"].as_str().unwrap_or_default()
}

fn severity_word(s: Severity) -> &'static str {
    match s {
        Severity::Low => "Low",
        Severity::Medium => "Medium",
        Severity::High => "High",
        Severity::Critical => "Critical",
    }
}

fn event_label(alert: &Alert) -> String {
    match alert.event {
        AlertEvent::Opened => "Incident opened".into(),
        AlertEvent::Updated => "Escalated".into(),
        AlertEvent::Resolved => "Resolved".into(),
        AlertEvent::Test => "Test".into(),
        AlertEvent::Summary => alert.rule.name.clone(),
    }
}

fn event_icon(alert: &Alert) -> &'static str {
    match alert.event {
        AlertEvent::Opened => "🚨",
        AlertEvent::Updated => "⚠️",
        AlertEvent::Resolved => "✅",
        AlertEvent::Test => "🧪",
        AlertEvent::Summary => "📊",
    }
}

fn title(alert: &Alert) -> String {
    format!("{} · {}", alert.rule.name, alert.program_label)
}

/// What broke, in one line: the top failing `program::instruction → error`.
fn root_cause(alert: &Alert) -> Option<String> {
    let fp = alert.payload["incident"]["evidence"]["fingerprints"].get(0)?;
    let error = fp["error"].as_str()?;
    let program = fp["program_name"].as_str().unwrap_or("program");
    let ix = fp["instruction"].as_str().map(|i| format!("::{i}")).unwrap_or_default();
    let share = fp["share"].as_f64().filter(|s| *s > 0.0).map(|s| format!(" ({:.0}% of failures)", s * 100.0));
    Some(format!("{program}{ix} → {error}{}", share.unwrap_or_default()))
}

/// Label/value pairs shown under the message.
fn facts(alert: &Alert) -> Vec<(String, String)> {
    if alert.event == AlertEvent::Summary {
        return alert.payload["facts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| Some((row[0].as_str()?.to_string(), row[1].as_str()?.to_string())))
            .collect();
    }
    let mut out = vec![
        ("Severity".to_string(), severity_word(alert.severity).to_string()),
        ("Program".to_string(), alert.program_label.clone()),
    ];
    if let Some(cause) = root_cause(alert) {
        out.push(("Root cause".into(), cause));
    }
    let inc = &alert.payload["incident"];
    if let Some(w) = inc["affected_wallets"].as_i64().filter(|w| *w > 0) {
        out.push(("Wallets affected".into(), w.to_string()));
    }
    if let Some(n) = inc["affected_count"].as_i64().filter(|n| *n > 0) {
        out.push(("Transactions".into(), n.to_string()));
    }
    if let Some(ms) = inc["detection_latency_ms"].as_i64() {
        out.push(("Detected in".into(), format!("{:.1}s", ms as f64 / 1000.0)));
    }
    if let Some(note) = inc["evidence"]["deploy"]["note"].as_str() {
        out.push(("Recent upgrade".into(), note.to_string()));
    }
    out
}

/// Facts without the two every alert starts with, which messages already show elsewhere.
fn details(alert: &Alert) -> Vec<(String, String)> {
    facts(alert).into_iter().filter(|(k, _)| k != "Severity" && k != "Program").collect()
}

/// Things worth doing, for reports.
fn next_actions(alert: &Alert) -> Vec<String> {
    alert.payload["next_actions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(String::from))
        .collect()
}

fn button_label(alert: &Alert) -> &'static str {
    if alert.event == AlertEvent::Summary {
        "Open dashboard"
    } else {
        "Investigate"
    }
}

fn color(alert: &Alert) -> u32 {
    match alert.event {
        AlertEvent::Resolved => 0x12B76A,
        AlertEvent::Test | AlertEvent::Summary => 0x2E90FA,
        _ => match alert.severity {
            Severity::Critical => 0xB42318,
            Severity::High => 0xD9480F,
            Severity::Medium => 0xCA8A04,
            Severity::Low => 0x475467,
        },
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn slack_escape(s: &str) -> String {
    html_escape(s)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn discord_body(alert: &Alert) -> Value {
    let link = incident_link(alert);
    let mut fields: Vec<Value> = details(alert)
        .into_iter()
        .map(|(k, v)| json!({ "name": k, "value": truncate(&v, 1000), "inline": alert.event != AlertEvent::Summary }))
        .take(20)
        .collect();
    let actions = next_actions(alert);
    if !actions.is_empty() {
        fields.push(json!({ "name": "Next steps", "value": truncate(&actions.iter().map(|a| format!("• {a}")).collect::<Vec<_>>().join("\n"), 1000) }));
    }
    json!({
        "username": "Vortex Sentinel",
        "embeds": [{
            "title": truncate(&format!("{} {}", event_icon(alert), title(alert)), 256),
            "description": truncate(&format!("**{}**\n{}", event_label(alert), alert.message), 4000),
            "url": if link.is_empty() { Value::Null } else { json!(link) },
            "color": color(alert),
            "fields": fields,
            "footer": { "text": format!("program {}", alert.program_id) },
            "timestamp": Utc::now().to_rfc3339(),
        }]
    })
}

fn slack_body(alert: &Alert) -> Value {
    let link = incident_link(alert);
    let summary = alert.event == AlertEvent::Summary;
    let fields: Vec<Value> = facts(alert)
        .into_iter()
        .map(|(k, v)| json!({ "type": "mrkdwn", "text": format!("*{k}*\n{}", slack_escape(&truncate(&v, 300))) }))
        .take(10)
        .collect();
    let mut blocks = vec![
        json!({ "type": "header", "text": { "type": "plain_text", "text": truncate(&format!("{} {}", event_icon(alert), title(alert)), 150), "emoji": true } }),
        json!({ "type": "section", "text": { "type": "mrkdwn", "text": format!("*{}*\n{}", event_label(alert), slack_escape(&truncate(&alert.message, 2800))) } }),
    ];
    if !fields.is_empty() {
        blocks.push(json!({ "type": "section", "fields": fields }));
    }
    let actions = next_actions(alert);
    if !actions.is_empty() {
        let text = actions.iter().map(|a| format!("• {}", slack_escape(a))).collect::<Vec<_>>().join("\n");
        blocks.push(json!({ "type": "section", "text": { "type": "mrkdwn", "text": format!("*Next steps*\n{}", truncate(&text, 2800)) } }));
    }
    if !link.is_empty() {
        let mut button = json!({ "type": "button", "text": { "type": "plain_text", "text": button_label(alert) }, "url": link });
        if !summary {
            button["style"] = json!("primary");
        }
        blocks.push(json!({ "type": "actions", "elements": [button] }));
    }
    json!({
        "text": format!("{} {} · {}", event_icon(alert), title(alert), alert.message),
        "attachments": [{ "color": format!("#{:06X}", color(alert)), "blocks": blocks }],
    })
}

fn telegram_body(alert: &Alert, chat_id: &str, reply_to: Option<&str>) -> Value {
    let link = incident_link(alert);
    let mut text = format!(
        "{} <b>{}</b>\n<i>{}</i>\n\n{}",
        event_icon(alert),
        html_escape(&title(alert)),
        html_escape(&event_label(alert)),
        html_escape(&truncate(&alert.message, 1500)),
    );
    let rows = details(alert);
    if !rows.is_empty() {
        text.push('\n');
        for (k, v) in &rows {
            text.push_str(&format!("\n<b>{}:</b> {}", html_escape(k), html_escape(&truncate(v, 300))));
        }
    }
    let actions = next_actions(alert);
    if !actions.is_empty() {
        text.push_str("\n\n<b>Next steps</b>");
        for a in actions {
            text.push_str(&format!("\n• {}", html_escape(&truncate(&a, 300))));
        }
    }
    let mut body = json!({
        "chat_id": chat_id,
        "text": truncate(&text, 4000),
        "parse_mode": "HTML",
        "disable_web_page_preview": true,
    });
    // Telegram only accepts public https links on buttons.
    if link.starts_with("https://") {
        body["reply_markup"] = json!({ "inline_keyboard": [[{ "text": button_label(alert), "url": link }]] });
    }
    if let Some(id) = reply_to.and_then(|r| r.parse::<i64>().ok()) {
        body["reply_to_message_id"] = json!(id);
        body["allow_sending_without_reply"] = json!(true);
    }
    body
}

fn pagerduty_body(alert: &Alert, routing_key: &str, dedup_key: &str, action: &str) -> Value {
    let link = incident_link(alert);
    let severity = match alert.severity {
        Severity::Critical => "critical",
        Severity::High => "error",
        Severity::Medium => "warning",
        Severity::Low => "info",
    };
    let prefix = if alert.event == AlertEvent::Test { "[TEST] " } else { "" };
    let mut body = json!({
        "routing_key": routing_key,
        "event_action": action,
        "dedup_key": dedup_key,
        "payload": {
            "summary": truncate(&format!("{prefix}{} — {}", title(alert), alert.message), 1000),
            "source": alert.program_label,
            "severity": severity,
            "timestamp": Utc::now().to_rfc3339(),
            "component": alert.program_id,
            "custom_details": {
                "facts": facts(alert).into_iter().map(|(k, v)| (k, json!(v))).collect::<serde_json::Map<_, _>>(),
                "rule": alert.rule.name,
            },
        },
    });
    if !link.is_empty() {
        body["links"] = json!([{ "href": link, "text": "Open in Sentinel" }]);
    }
    body
}
