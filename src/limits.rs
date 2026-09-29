//! Guards for a publicly hosted instance: per-account caps, operator-only
//! settings and per-IP rate limits on anything that writes.

use crate::engine::Sentinel;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

pub struct Limits {
    pub max_watched: usize,
    pub max_rules: usize,
    auth_per_min: u32,
    writes_per_min: u32,
    admins: HashSet<String>,
    trust_proxy: bool,
    windows: Mutex<HashMap<(String, &'static str), (i64, u32)>>,
}

impl Limits {
    pub fn from_env() -> Self {
        Self {
            max_watched: env_usize("SENTINEL_MAX_WATCHED", 10),
            max_rules: env_usize("SENTINEL_MAX_RULES", 25),
            auth_per_min: env_usize("SENTINEL_AUTH_PER_MIN", 20) as u32,
            writes_per_min: env_usize("SENTINEL_WRITES_PER_MIN", 60) as u32,
            admins: std::env::var("SENTINEL_ADMINS")
                .unwrap_or_default()
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            trust_proxy: std::env::var("SENTINEL_TRUST_PROXY").is_ok_and(|v| v == "1" || v == "true"),
            windows: Mutex::new(HashMap::new()),
        }
    }

    /// Operators (`SENTINEL_ADMINS`) may change shared settings and skip caps.
    pub fn is_admin(&self, account: &str) -> bool {
        self.admins.contains(account)
    }

    /// Fixed one-minute window per (client, kind). Returns false when over the limit.
    fn allow(&self, client: &str, kind: &'static str, per_min: u32) -> bool {
        let minute = chrono::Utc::now().timestamp() / 60;
        let mut w = self.windows.lock().unwrap();
        if w.len() > 50_000 {
            w.retain(|_, (m, _)| *m == minute);
        }
        let entry = w.entry((client.to_string(), kind)).or_insert((minute, 0));
        if entry.0 != minute {
            *entry = (minute, 0);
        }
        entry.1 += 1;
        entry.1 <= per_min
    }

    fn client(&self, req: &Request) -> String {
        if self.trust_proxy {
            if let Some(ip) = req
                .headers()
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(',').next())
            {
                return ip.trim().to_string();
            }
        }
        req.extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip().to_string())
            .unwrap_or_else(|| "unknown".into())
    }
}

/// Rate-limits sign-in and every state-changing request per client IP.
pub async fn rate_limit(State(s): State<Arc<Sentinel>>, req: Request, next: Next) -> Response {
    let path = req.uri().path();
    let (kind, per_min) = if path.starts_with("/api/auth/") && req.method() == Method::POST {
        ("auth", s.limits.auth_per_min)
    } else if path.starts_with("/api/") && !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
        ("write", s.limits.writes_per_min)
    } else {
        return next.run(req).await;
    };
    let client = s.limits.client(&req);
    if !s.limits.allow(&client, kind, per_min) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({ "error": "Too many requests; wait a minute and try again" })),
        )
            .into_response();
    }
    next.run(req).await
}
