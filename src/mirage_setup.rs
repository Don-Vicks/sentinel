//! Sets Mirage up by itself. Given a Solami key that may manage Mirage, Sentinel finds or creates
//! one subscription labelled "sentinel", keeps its filter equal to the programs being watched
//! (including failed transactions), and works out the stream URL. Nobody pastes a URL.
//!
//! The key needs the `MirageView`, `MirageManage` and `MirageStream` permissions. Without them
//! this stays off and gRPC failover is simply unavailable; nothing else is affected.

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::Duration;
use vortex::geyser::stream::StreamFilters;

const DEFAULT_API: &str = "https://api.solami.dev";
const DEFAULT_WS: &str = "wss://ws.solami.dev";
const LABEL: &str = "sentinel";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// No usable key, or the key may not manage Mirage.
    Off,
    /// A subscription exists and matches the watched programs.
    Ready,
}

/// What the rest of the app reads: the stream URL (a secret, never shown) and the state.
pub struct Handle {
    url: RwLock<Option<String>>,
    state: Mutex<State>,
}

impl Handle {
    fn new() -> Self {
        Self { url: RwLock::new(None), state: Mutex::new(State::Off) }
    }
    pub fn url(&self) -> Option<String> {
        self.url.read().unwrap().clone()
    }
    pub fn state(&self) -> State {
        *self.state.lock().unwrap()
    }
    fn set(&self, url: Option<String>) {
        *self.state.lock().unwrap() = if url.is_some() { State::Ready } else { State::Off };
        *self.url.write().unwrap() = url;
    }
    /// Waits for the URL to exist (provisioning happens in the background at startup).
    pub async fn wait_url(&self, up_to: Duration) -> Option<String> {
        let deadline = tokio::time::Instant::now() + up_to;
        loop {
            if let Some(u) = self.url() {
                return Some(u);
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}

pub fn handle() -> &'static std::sync::Arc<Handle> {
    static H: OnceLock<std::sync::Arc<Handle>> = OnceLock::new();
    H.get_or_init(|| std::sync::Arc::new(Handle::new()))
}

/// Talks to Solami's Mirage management API with one key.
pub struct Mirage {
    http: reqwest::Client,
    api: String,
    ws: String,
    key: String,
    id: Mutex<Option<String>>,
    synced: Mutex<Vec<String>>,
}

impl Mirage {
    pub fn new(api: &str, ws: &str, key: &str) -> Self {
        Self {
            http: reqwest::Client::builder().timeout(Duration::from_secs(15)).build().expect("reqwest client"),
            api: api.trim_end_matches('/').into(),
            ws: ws.trim_end_matches('/').into(),
            key: key.into(),
            id: Mutex::new(None),
            synced: Mutex::new(vec![]),
        }
    }

    async fn call(&self, path: &str, body: Value) -> Result<Value> {
        let res = self.http.post(format!("{}{path}", self.api)).header("x-api-key", &self.key).json(&body).send().await?;
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        if !status.is_success() {
            let msg = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["message"].as_str().map(String::from)).unwrap_or(text);
            bail!("Mirage {path}: HTTP {} {}", status.as_u16(), msg);
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    fn filter(programs: &[String]) -> Value {
        json!({ "account_include": programs, "vote": false, "failed": true, "commitment": "processed" })
    }

    /// The subscriptions in a list reply, whichever way it is wrapped.
    fn items(v: &Value) -> Vec<&Value> {
        let arr = v.as_array().or_else(|| ["subscriptions", "data", "items", "result"].iter().find_map(|k| v[*k].as_array()));
        arr.map(|a| a.iter().collect()).unwrap_or_default()
    }

    /// Finds or creates the subscription and returns its stream URL. An error that says the key
    /// lacks permission means Mirage is simply unavailable to this key.
    pub async fn ensure(&self, programs: &[String]) -> Result<String> {
        if programs.is_empty() {
            bail!("no programs to subscribe to yet");
        }
        let listed = self.call("/mirage/list", json!({})).await?;
        let existing = Self::items(&listed).into_iter().find(|s| s["label"] == LABEL).and_then(|s| s["id"].as_str().map(String::from));
        let id = match existing {
            Some(id) => {
                // Make sure it watches what we watch now.
                self.call("/mirage/update", json!({ "id": id, "filter": Self::filter(programs), "enabled": true })).await?;
                id
            }
            None => {
                let made = self.call("/mirage/create", json!({ "label": LABEL, "filter": Self::filter(programs) })).await?;
                made["id"].as_str().map(String::from).ok_or_else(|| anyhow!("Mirage create returned no id"))?
            }
        };
        *self.id.lock().unwrap() = Some(id.clone());
        *self.synced.lock().unwrap() = sorted(programs);
        Ok(format!("{}/mirage/stream/{id}?api_key={}", self.ws, self.key))
    }

    /// Points the subscription at a new set of programs. A no-op when nothing changed.
    pub async fn sync(&self, programs: &[String]) -> Result<bool> {
        let Some(id) = self.id.lock().unwrap().clone() else { return Ok(false) };
        if programs.is_empty() || *self.synced.lock().unwrap() == sorted(programs) {
            return Ok(false);
        }
        self.call("/mirage/update", json!({ "id": id, "filter": Self::filter(programs) })).await?;
        *self.synced.lock().unwrap() = sorted(programs);
        Ok(true)
    }
}

fn sorted(p: &[String]) -> Vec<String> {
    let mut v = p.to_vec();
    v.sort();
    v
}

fn is_permission_error(e: &anyhow::Error) -> bool {
    let t = e.to_string();
    t.contains("HTTP 401") || t.contains("HTTP 403")
}

/// Keys to try, most specific first.
fn candidate_keys() -> Vec<String> {
    let mut keys = Vec::new();
    for var in ["MIRAGE_API_KEY", "BLUR_API_KEY", "YELLOWSTONE_TOKEN"] {
        if let Ok(k) = std::env::var(var) {
            if !k.is_empty() && !keys.contains(&k) {
                keys.push(k);
            }
        }
    }
    keys
}

/// Background task: provision Mirage once programs exist, then follow the watchlist.
pub async fn run(mut filters: tokio::sync::watch::Receiver<StreamFilters>) {
    let h = handle();
    if let Some(url) = std::env::var("MIRAGE_STREAM_URL").ok().filter(|u| !u.is_empty()) {
        h.set(Some(url)); // a manually supplied stream, as before
        return;
    }
    if std::env::var("SENTINEL_MIRAGE").is_ok_and(|v| v == "off") {
        return;
    }
    let api = std::env::var("MIRAGE_API_URL").unwrap_or_else(|_| DEFAULT_API.into());
    let ws = std::env::var("MIRAGE_WS_URL").unwrap_or_else(|_| DEFAULT_WS.into());
    let keys = candidate_keys();
    if keys.is_empty() {
        return;
    }
    let mut manager: Option<Mirage> = None;
    let mut warned = false;
    loop {
        let programs = filters.borrow().programs.clone();
        if manager.is_none() && !programs.is_empty() {
            for key in &keys {
                let m = Mirage::new(&api, &ws, key);
                match m.ensure(&programs).await {
                    Ok(url) => {
                        tracing::info!("Mirage failover is ready (subscription \"{LABEL}\")");
                        h.set(Some(url));
                        manager = Some(m);
                        break;
                    }
                    Err(e) if is_permission_error(&e) => continue,
                    Err(e) => {
                        tracing::warn!(error = %crate::redact::scrub(&e.to_string()), "Mirage setup failed; will retry");
                        break;
                    }
                }
            }
            if manager.is_none() && !warned {
                warned = true;
                tracing::info!("Mirage failover is off: no key has the MirageView, MirageManage and MirageStream permissions");
            }
        } else if let Some(m) = &manager {
            match m.sync(&programs).await {
                Ok(true) => tracing::info!(programs = programs.len(), "Mirage subscription follows the watchlist"),
                Ok(false) => {}
                Err(e) => tracing::warn!(error = %crate::redact::scrub(&e.to_string()), "Mirage sync failed"),
            }
        }
        // Wait for the watchlist to change; retry provisioning now and then if it hasn't worked.
        let wait = if manager.is_none() { Duration::from_secs(120) } else { Duration::from_secs(3600) };
        match tokio::time::timeout(wait, filters.changed()).await {
            Ok(Ok(())) => tokio::time::sleep(Duration::from_secs(2)).await, // let a burst of changes settle
            Ok(Err(_)) => return,
            Err(_) => {}
        }
    }
}
