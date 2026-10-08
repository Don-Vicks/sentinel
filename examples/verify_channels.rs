//! Sends one real test message through each alert channel you configure, using Sentinel's own
//! delivery code, and reports what each service answered. Run it on your machine with your
//! own credentials; nothing is stored anywhere but a throwaway database.
//!
//!     SLACK_WEBHOOK_URL=https://hooks.slack.com/services/…  \
//!     SLACK_BOT_TOKEN=xoxb-… SLACK_CHANNEL=#alerts  \
//!     TELEGRAM_BOT_TOKEN=123:ABC TELEGRAM_CHAT_ID=-100…      \
//!     PAGERDUTY_ROUTING_KEY=…  DISCORD_WEBHOOK_URL=…  WEBHOOK_URL=…  \
//!     cargo run --example verify_channels
//!
//! PagerDuty gets a trigger and then a resolve, so nobody stays paged. Exits non-zero if any
//! channel fails. Set only the ones you use.

use chrono::Utc;
use sentinel::engine::Sentinel;
use sentinel::model::*;
use sentinel::source::VortexSource;
use sentinel::store::Store;
use std::sync::Arc;
use tokio::sync::broadcast;
use vortex::events::VortexTransaction;
use vortex::hub::HubStats;

struct Quiet(broadcast::Sender<Arc<VortexTransaction>>);
impl VortexSource for Quiet {
    fn subscribe(&self) -> broadcast::Receiver<Arc<VortexTransaction>> {
        self.0.subscribe()
    }
    fn watch_programs(&self, _: Vec<String>) {}
    fn health(&self) -> HubStats {
        HubStats { transactions: 0, last_slot: 0, last_transaction_at: None, started_at: Utc::now(), programs: vec![], subscribers: 0 }
    }
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut channels = Vec::new();
    let mut add = |kind| channels.push(Channel { kind, min_severity: None });
    if let Some(url) = env("SLACK_WEBHOOK_URL") {
        add(ChannelKind::Slack { url });
    }
    if let (Some(bot_token), Some(channel)) = (env("SLACK_BOT_TOKEN"), env("SLACK_CHANNEL")) {
        add(ChannelKind::SlackBot { bot_token, channel });
    }
    if let (Some(bot_token), Some(chat_id)) = (env("TELEGRAM_BOT_TOKEN"), env("TELEGRAM_CHAT_ID")) {
        add(ChannelKind::Telegram { bot_token, chat_id });
    }
    if let Some(routing_key) = env("PAGERDUTY_ROUTING_KEY") {
        add(ChannelKind::Pagerduty { routing_key });
    }
    if let Some(url) = env("DISCORD_WEBHOOK_URL") {
        add(ChannelKind::Discord { url });
    }
    if let Some(url) = env("WEBHOOK_URL") {
        add(ChannelKind::Webhook { url });
    }
    if channels.is_empty() {
        eprintln!("Nothing to test. Set SLACK_WEBHOOK_URL, TELEGRAM_BOT_TOKEN + TELEGRAM_CHAT_ID, PAGERDUTY_ROUTING_KEY, DISCORD_WEBHOOK_URL or WEBHOOK_URL.");
        std::process::exit(2);
    }

    let db = std::env::temp_dir().join(format!("sentinel-channels-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let store = Arc::new(Store::open(db.to_str().unwrap())?);
    let (bus, _) = broadcast::channel(4);
    let s = Sentinel::new(store.clone(), Arc::new(Quiet(bus)), None, sentinel::pricing::PriceBook::new(), "http://localhost:8080".into())?;
    let rule = store.create_rule(AlertRule {
        id: 0,
        owner: None,
        name: "Channel check".into(),
        program_id: None,
        condition: Condition::System { kinds: vec![] },
        create_incident: false,
        severity: Severity::High,
        webhook_url: None,
        channels: channels.clone(),
        enabled: true,
        cooldown_secs: 0,
        created_at: Utc::now(),
        last_fired_at: None,
    })?;
    println!("Sending a test through {} channel(s)…", channels.len());
    s.test_rule(rule);

    // Each channel makes one delivery, except PagerDuty: a trigger, then a resolve unless the trigger failed.
    let settled = |seen: &[AlertExecution]| {
        channels.iter().all(|c| {
            let label = match c.kind {
                ChannelKind::Pagerduty { .. } => "pagerduty",
                ChannelKind::Slack { .. } => "slack",
                ChannelKind::SlackBot { .. } => "slack app",
                ChannelKind::Telegram { .. } => "telegram",
                ChannelKind::Discord { .. } => "discord",
                ChannelKind::Webhook { .. } => "webhook",
            };
            let mine: Vec<_> = seen.iter().filter(|e| e.channel.as_deref().unwrap_or("webhook") == label).collect();
            match label {
                "pagerduty" => mine.len() >= 2 || mine.iter().any(|e| !e.delivered),
                _ => !mine.is_empty(),
            }
        })
    };
    let mut seen = Vec::new();
    for _ in 0..60 {
        seen = store.executions(100)?;
        if settled(&seen) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    let mut failed = false;
    for e in seen.iter().rev() {
        let what = e.channel.clone().unwrap_or_else(|| "webhook".into());
        if e.delivered {
            println!("  ok      {what:<10} {} in {} ms", e.status_code.map(|c| format!("HTTP {c}")).unwrap_or_default(), e.latency_ms.unwrap_or(0));
        } else {
            failed = true;
            println!("  FAILED  {what:<10} {}", e.error.as_deref().unwrap_or("no reason given"));
        }
    }
    if !settled(&seen) {
        failed = true;
        println!("  FAILED  some deliveries had not finished after 30 s");
    }
    let _ = std::fs::remove_file(&db);
    if failed {
        std::process::exit(1);
    }
    println!("All channels answered. Check each one for the message.");
    Ok(())
}
