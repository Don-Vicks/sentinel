//! Webhook delivery for fired alert rules. Discord and Slack URLs get their
//! native message format; anything else receives the raw JSON payload.

use crate::live::LiveEvent;
use crate::model::{AlertExecution, AlertRule};
use crate::store::Store;
use chrono::Utc;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

const ATTEMPTS: u32 = 3;

pub struct Alert {
    pub rule: AlertRule,
    pub program_id: String,
    pub program_label: String,
    pub message: String,
    pub incident_id: Option<i64>,
    pub payload: Value,
}

#[derive(Clone)]
pub struct Dispatcher {
    client: reqwest::Client,
    store: Arc<Store>,
    live: broadcast::Sender<Arc<LiveEvent>>,
    allow_private: bool,
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

impl Dispatcher {
    pub fn allow_private(&self) -> bool {
        self.allow_private
    }

    pub fn new(store: Arc<Store>, live: broadcast::Sender<Arc<LiveEvent>>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .user_agent("vortex-sentinel/0.1")
            .build()
            .expect("reqwest client");
        let allow_private = std::env::var("SENTINEL_ALLOW_PRIVATE_WEBHOOKS").is_ok_and(|v| v == "1" || v == "true");
        Self {
            client,
            store,
            live,
            allow_private,
        }
    }

    /// Records the alert and delivers it in the background.
    pub fn dispatch(&self, alert: Alert) {
        let this = self.clone();
        tokio::spawn(async move {
            let exec = this.deliver(alert).await;
            match this.store.record_execution(exec) {
                Ok(exec) => {
                    let _ = this.live.send(Arc::new(LiveEvent::Alert { execution: exec }));
                }
                Err(e) => tracing::error!(error = %e, "failed to record alert execution"),
            }
        });
    }

    async fn deliver(&self, alert: Alert) -> AlertExecution {
        let mut exec = AlertExecution {
            id: 0,
            owner: alert.rule.owner.clone(),
            rule_id: alert.rule.id,
            rule_name: alert.rule.name.clone(),
            program_id: alert.program_id.clone(),
            fired_at: Utc::now(),
            message: alert.message.clone(),
            incident_id: alert.incident_id,
            webhook_url: alert.rule.webhook_url.clone(),
            delivered: false,
            status_code: None,
            error: None,
            latency_ms: None,
            attempts: 0,
        };
        let Some(url) = alert.rule.webhook_url.clone().filter(|u| !u.is_empty()) else {
            // Incident-only rule: nothing to deliver.
            exec.delivered = true;
            return exec;
        };
        if let Err(e) = check_webhook_url(&url, self.allow_private).await {
            exec.error = Some(crate::redact::scrub(&e.to_string()));
            return exec;
        }
        let body = format_body(&url, &alert);
        let delivery_id = uuid::Uuid::new_v4().to_string();

        for attempt in 1..=ATTEMPTS {
            exec.attempts = attempt;
            let started = Instant::now();
            let res = self
                .client
                .post(&url)
                .header("X-Sentinel-Delivery", &delivery_id)
                .header("X-Sentinel-Event", "alert")
                .json(&body)
                .send()
                .await;
            exec.latency_ms = Some(started.elapsed().as_millis() as i64);
            match res {
                Ok(resp) => {
                    let status = resp.status();
                    exec.status_code = Some(status.as_u16());
                    if status.is_success() {
                        exec.delivered = true;
                        exec.error = None;
                        return exec;
                    }
                    exec.error = Some(format!("HTTP {status}"));
                    // Client errors won't fix themselves, except rate limits.
                    if status.is_client_error() && status.as_u16() != 429 {
                        return exec;
                    }
                }
                Err(e) => exec.error = Some(crate::redact::scrub(&e.to_string())),
            }
            tokio::time::sleep(Duration::from_millis(500 * 3u64.pow(attempt - 1))).await;
        }
        exec
    }
}

fn format_body(url: &str, alert: &Alert) -> Value {
    let link = alert.payload["links"]["incident"].as_str().unwrap_or_default();
    if url.contains("discord.com/api/webhooks") || url.contains("discordapp.com/api/webhooks") {
        json!({
            "username": "Vortex Sentinel",
            "embeds": [{
                "title": format!("{} · {}", alert.rule.name, alert.program_label),
                "description": alert.message,
                "url": if link.is_empty() { Value::Null } else { json!(link) },
                "color": severity_color(&alert.payload),
                "footer": { "text": format!("program {}", alert.program_id) },
                "timestamp": Utc::now().to_rfc3339(),
            }]
        })
    } else if url.contains("hooks.slack.com") {
        json!({ "text": format!("*{}* · {}\n{}\n{}", alert.rule.name, alert.program_label, alert.message, link) })
    } else {
        alert.payload.clone()
    }
}

fn severity_color(payload: &Value) -> u32 {
    match payload["severity"].as_str() {
        Some("critical") => 0xB42318,
        Some("high") => 0xD9480F,
        Some("medium") => 0xCA8A04,
        _ => 0x475467,
    }
}
