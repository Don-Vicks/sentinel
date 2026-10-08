//! The other direction of the Telegram channel: ask Sentinel things from the chat that
//! receives its alerts, and acknowledge an incident with one tap.
//!
//! Sentinel polls `getUpdates` for each bot token used by an alert rule. A chat is only
//! answered if a rule already sends to it with that token, and it can only see and change
//! the programs that rule's owner watches.

use crate::engine::Sentinel;
use crate::model::{ChannelKind, IncidentStatus};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

const API: &str = "https://api.telegram.org";

fn api_base() -> String {
    std::env::var("SENTINEL_TELEGRAM_API").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| API.to_string()).trim_end_matches('/').to_string()
}

/// A chat Sentinel may talk to: who owns the rule that sends there.
#[derive(Debug, Clone, PartialEq)]
pub struct Authorised {
    /// `None` is an old rule from before accounts, which may act on everything.
    pub owners: Vec<Option<String>>,
}

/// For each bot token used by an enabled rule: its chats, and the owners of the rules that use them.
pub fn bots(s: &Sentinel) -> HashMap<String, Vec<(String, Option<String>)>> {
    let mut out: HashMap<String, Vec<(String, Option<String>)>> = HashMap::new();
    let mut add = |channels: &[crate::model::Channel], owner: Option<String>| {
        for ch in channels {
            if let ChannelKind::Telegram { bot_token, chat_id } = &ch.kind {
                out.entry(bot_token.clone()).or_default().push((chat_id.trim().to_string(), owner.clone()));
            }
        }
    };
    for rule in s.store.rules().unwrap_or_default().into_iter().filter(|r| r.enabled) {
        add(&rule.channels, rule.owner.clone());
    }
    for schedule in s.store.schedules().unwrap_or_default().into_iter().filter(|x| x.enabled) {
        add(&schedule.channels, Some(schedule.owner.clone()));
    }
    for list in out.values_mut() {
        list.sort();
        list.dedup();
    }
    out
}

/// Whether a chat (numeric id and, for channels, @username) is one a rule sends to, and who owns it.
fn authorise(chats: &[(String, Option<String>)], chat_id: i64, username: Option<&str>) -> Option<Authorised> {
    let owners: Vec<Option<String>> = chats
        .iter()
        .filter(|(configured, _)| {
            configured.parse::<i64>().is_ok_and(|c| c == chat_id)
                || configured.strip_prefix('@').zip(username).is_some_and(|(c, u)| c.eq_ignore_ascii_case(u))
        })
        .map(|(_, owner)| owner.clone())
        .collect();
    (!owners.is_empty()).then_some(Authorised { owners })
}

/// Programs these owners watch (everything, for an ownerless rule).
fn scope(s: &Sentinel, auth: &Authorised) -> Vec<(String, String)> {
    let all = s.programs();
    all.into_iter()
        .filter(|p| auth.owners.iter().any(|o| o.as_deref().is_none_or(|o| s.is_watching(o, &p.program_id))))
        .map(|p| (p.program_id, p.label))
        .collect()
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// "30m", "2h", "1d" as minutes.
pub fn parse_minutes(text: &str) -> Option<u32> {
    let text = text.trim().to_lowercase();
    let split = text.find(|c: char| !c.is_ascii_digit())?;
    let n: u32 = text[..split].parse().ok().filter(|n| *n > 0)?;
    match &text[split..] {
        "m" | "min" => Some(n),
        "h" | "hr" => Some(n * 60),
        "d" => Some(n * 1440),
        _ => None,
    }
}

const HELP: &str = "<b>Vortex Sentinel</b>\n\
/status: every program, its health and open incidents\n\
/incidents: what is open right now\n\
/health [program]: the score and what is pulling it down\n\
/summary [program] [24h|7d]: what it did\n\
/mute [program] 30m: hold notifications for a deploy\n\
/unmute [program]\n\
/ack &lt;id&gt;: mark an incident as being investigated\n\
/resolve &lt;id&gt;";

/// Picks a program from the words after a command, or explains why it can't.
fn pick(programs: &[(String, String)], words: &[&str]) -> Result<(String, String, usize), String> {
    for (n, w) in words.iter().enumerate().take(1) {
        let lw = w.to_lowercase();
        if let Some((id, label)) = programs.iter().find(|(id, label)| id == w || label.to_lowercase() == lw) {
            return Ok((id.clone(), label.clone(), n + 1));
        }
    }
    match programs {
        [(id, label)] => Ok((id.clone(), label.clone(), 0)),
        [] => Err("You don't watch any programs yet.".into()),
        many => Err(format!("Which program? One of: {}", many.iter().map(|(_, l)| esc(l)).collect::<Vec<_>>().join(", "))),
    }
}

/// The answer to one command, as Telegram HTML.
pub async fn command(s: &Arc<Sentinel>, auth: &Authorised, text: &str) -> String {
    let mut words = text.split_whitespace();
    let cmd = words.next().unwrap_or_default().split('@').next().unwrap_or_default().to_lowercase();
    let rest: Vec<&str> = words.collect();
    let programs = scope(s, auth);
    match cmd.as_str() {
        "/start" | "/help" => HELP.to_string(),
        "/status" => {
            if programs.is_empty() {
                return "You don't watch any programs yet.".into();
            }
            let mut out = String::new();
            for snap in s.programs().into_iter().filter(|p| programs.iter().any(|(id, _)| *id == p.program_id)) {
                let health = s.health(&snap.program_id);
                let score = health.as_ref().and_then(|h| h.score).map(|v| format!("{v}/100")).unwrap_or_else(|| "learning".into());
                let muted = s.program_detail(&snap.program_id).is_some_and(|d| d["program"]["muted_until"].as_str().is_some_and(|t| t > chrono::Utc::now().to_rfc3339().as_str()));
                out.push_str(&format!(
                    "<b>{}</b>: {score} · {:.1} tx/s · {:.1}% failing · {} open incident{}{}\n",
                    esc(&snap.label),
                    snap.tps_10s,
                    snap.failure_rate_60s,
                    snap.open_incidents,
                    if snap.open_incidents == 1 { "" } else { "s" },
                    if muted { " · muted" } else { "" }
                ));
            }
            out
        }
        "/incidents" => {
            let open: Vec<_> = s
                .store
                .incidents(None, 300)
                .unwrap_or_default()
                .into_iter()
                .filter(|i| i.status != IncidentStatus::Resolved && programs.iter().any(|(id, _)| *id == i.program_id))
                .take(10)
                .collect();
            if open.is_empty() {
                return "Nothing is open. ✅".into();
            }
            open.iter()
                .map(|i| format!("#{} <b>{:?}</b> {}\n{}", i.id, i.severity, esc(&i.title), esc(&i.summary)))
                .collect::<Vec<_>>()
                .join("\n\n")
        }
        "/health" => match pick(&programs, &rest) {
            Err(e) => e,
            Ok((id, label, _)) => match s.health(&id) {
                None => "That program isn't being monitored.".into(),
                Some(h) => {
                    let mut out = format!("<b>{}</b>: {}\n{}\n", esc(&label), h.score.map(|v| format!("{v}/100 ({})", h.status)).unwrap_or_else(|| "still learning".into()), esc(&h.headline));
                    for c in h.checks.iter().filter(|c| matches!(c.status, crate::health::Status::Warn | crate::health::Status::Fail)) {
                        out.push_str(&format!("• <b>{}</b>: {}\n", esc(c.label), esc(&c.detail)));
                    }
                    out
                }
            },
        },
        "/summary" => match pick(&programs, &rest) {
            Err(e) => e,
            Ok((id, label, used)) => {
                let period = rest.get(used).and_then(|p| crate::api::parse_period(Some(p))).unwrap_or(86_400);
                match s.summary(&id, period) {
                    Err(e) => esc(&e.to_string()),
                    Ok(sum) => {
                        let mut out = format!("📊 <b>{}</b>\n{}\n", esc(&label), esc(&sum.headline));
                        for (k, v) in sum.facts() {
                            out.push_str(&format!("\n<b>{}:</b> {}", esc(&k), esc(&v)));
                        }
                        out
                    }
                }
            }
        },
        "/mute" => match pick(&programs, &rest) {
            Err(e) => e,
            Ok((id, label, used)) => match rest.get(used).and_then(|d| parse_minutes(d)) {
                None => "Say how long, for example <code>/mute 30m</code> or <code>/mute pump 2h</code>.".into(),
                Some(minutes) => match s.set_mute(&id, minutes, Some("Muted from Telegram".into())) {
                    Ok(_) => format!("🔕 {} is muted for {minutes} minutes. Incidents are still recorded.", esc(&label)),
                    Err(e) => esc(&e.to_string()),
                },
            },
        },
        "/unmute" => match pick(&programs, &rest) {
            Err(e) => e,
            Ok((id, label, _)) => match s.set_mute(&id, 0, None) {
                Ok(_) => format!("🔔 {} is live again.", esc(&label)),
                Err(e) => esc(&e.to_string()),
            },
        },
        "/ack" | "/resolve" => {
            let Some(id) = rest.first().and_then(|w| w.trim_start_matches('#').parse::<i64>().ok()) else {
                return format!("Give the incident number, for example <code>{cmd} 1001</code>.");
            };
            let status = if cmd == "/ack" { IncidentStatus::Investigating } else { IncidentStatus::Resolved };
            set_status(s, &programs, id, status)
        }
        _ => format!("I don't know that one.\n\n{HELP}"),
    }
}

/// Changes an incident's status if it belongs to a program this chat may act on.
fn set_status(s: &Sentinel, programs: &[(String, String)], id: i64, status: IncidentStatus) -> String {
    match s.store.incident(id) {
        Ok(Some(inc)) if programs.iter().any(|(p, _)| *p == inc.program_id) => match s.set_incident_status(id, status) {
            Ok(_) if status == IncidentStatus::Resolved => format!("✅ Incident #{id} marked resolved."),
            Ok(_) => format!("👀 Incident #{id} is being investigated."),
            Err(e) => esc(&e.to_string()),
        },
        _ => format!("I can't find incident #{id} among your programs."),
    }
}

async fn call(client: &reqwest::Client, api: &str, token: &str, method: &str, body: Value) -> Result<Value> {
    let res = client.post(format!("{api}/bot{token}/{method}")).json(&body).send().await.map_err(|e| anyhow::anyhow!(crate::redact::scrub(&e.without_url().to_string())))?;
    Ok(res.json().await.unwrap_or(Value::Null))
}

/// Fetches and answers one batch of updates. Returns the offset to ask for next.
pub async fn poll_once(s: &Arc<Sentinel>, client: &reqwest::Client, token: &str, offset: i64, wait_secs: u64) -> Result<i64> {
    let api = api_base();
    let reply = call(client, &api, token, "getUpdates", json!({ "offset": offset, "timeout": wait_secs, "allowed_updates": ["message", "callback_query"] })).await?;
    let mut next = offset;
    for update in reply["result"].as_array().into_iter().flatten() {
        next = next.max(update["update_id"].as_i64().unwrap_or(0) + 1);
        let chats = bots(s).remove(token).unwrap_or_default();
        // A message, or the tap of a button on one.
        let (chat, text, callback) = if let Some(m) = update.get("message") {
            (&m["chat"], m["text"].as_str().unwrap_or_default().to_string(), None)
        } else if let Some(q) = update.get("callback_query") {
            (&q["message"]["chat"], String::new(), Some(q))
        } else {
            continue;
        };
        let Some(chat_id) = chat["id"].as_i64() else { continue };
        let Some(auth) = authorise(&chats, chat_id, chat["username"].as_str()) else {
            // Strangers get nothing, not even an error.
            continue;
        };
        if let Some(q) = callback {
            let data = q["data"].as_str().unwrap_or_default();
            let who = q["from"]["first_name"].as_str().unwrap_or("Someone").to_string();
            let programs = scope(s, &auth);
            let (answer, announce) = match data.strip_prefix("ack:").and_then(|id| id.parse::<i64>().ok()) {
                Some(id) => {
                    let result = set_status(s, &programs, id, IncidentStatus::Investigating);
                    (result.clone(), result.starts_with('👀').then(|| format!("👀 {} is investigating incident #{id}.", esc(&who))))
                }
                None => ("Unknown action".to_string(), None),
            };
            let _ = call(client, &api, token, "answerCallbackQuery", json!({ "callback_query_id": q["id"], "text": answer.chars().take(180).collect::<String>().replace("<b>", "").replace("</b>", "") })).await;
            if let Some(text) = announce {
                let _ = call(client, &api, token, "sendMessage", json!({ "chat_id": chat_id, "text": text, "parse_mode": "HTML", "reply_to_message_id": q["message"]["message_id"] })).await;
            }
            continue;
        }
        if !text.starts_with('/') {
            continue;
        }
        let answer = command(s, &auth, &text).await;
        let _ = call(client, &api, token, "sendMessage", json!({ "chat_id": chat_id, "text": answer.chars().take(4000).collect::<String>(), "parse_mode": "HTML", "disable_web_page_preview": true })).await;
    }
    Ok(next)
}

/// Keeps one poller per bot token in use, starting and stopping them as rules change.
pub async fn run(s: Arc<Sentinel>) {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(40)).user_agent("vortex-sentinel/0.1").build().expect("http client");
    let mut running: HashSet<String> = HashSet::new();
    let alive = Arc::new(std::sync::Mutex::new(HashSet::<String>::new()));
    loop {
        let wanted: HashSet<String> = bots(&s).into_keys().collect();
        let fresh: Vec<String> = wanted.iter().filter(|t| !running.contains(*t)).cloned().collect();
        for token in fresh {
            running.insert(token.clone());
            alive.lock().unwrap().insert(token.clone());
            let (s, client, token, alive) = (s.clone(), client.clone(), token, alive.clone());
            tokio::spawn(async move {
                let mut offset = 0;
                let mut failures = 0u32;
                while bots(&s).contains_key(&token) {
                    match poll_once(&s, &client, &token, offset, 25).await {
                        Ok(next) => {
                            offset = next;
                            failures = 0;
                        }
                        Err(_) => {
                            failures += 1;
                            tokio::time::sleep(Duration::from_secs((2u64 << failures.min(5)).min(60))).await;
                        }
                    }
                }
                alive.lock().unwrap().remove(&token);
            });
        }
        // A poller that ended (its rules were removed) can be started again later.
        running.retain(|t| alive.lock().unwrap().contains(t));
        tokio::time::sleep(Duration::from_secs(15)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_minutes("30m"), Some(30));
        assert_eq!(parse_minutes("2h"), Some(120));
        assert_eq!(parse_minutes("1D"), Some(1440));
        assert_eq!(parse_minutes("soon"), None);
        assert_eq!(parse_minutes("0m"), None);
        assert_eq!(parse_minutes("30"), None);
    }

    #[test]
    fn only_chats_a_rule_sends_to_are_answered() {
        let chats = vec![("-1001".to_string(), Some("A".to_string())), ("@alerts_room".to_string(), Some("B".to_string()))];
        assert_eq!(authorise(&chats, -1001, None).unwrap().owners, vec![Some("A".to_string())]);
        assert_eq!(authorise(&chats, 5, Some("Alerts_Room")).unwrap().owners, vec![Some("B".to_string())]);
        assert!(authorise(&chats, 42, None).is_none(), "a stranger");
        assert!(authorise(&chats, 5, Some("someone_else")).is_none());
    }
}
