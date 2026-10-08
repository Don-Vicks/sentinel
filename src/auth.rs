//! Sign-In With Solana. The server issues a one-time message with a nonce;
//! the wallet signs it; the ed25519 signature proves control of the account.
//! Sessions are random tokens in an HttpOnly cookie, stored hashed.

use crate::store::Store;
use anyhow::{anyhow, bail, Result};
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{header, StatusCode};
use chrono::{DateTime, Duration, Utc};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use std::collections::HashMap;
use std::convert::Infallible;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

pub const COOKIE: &str = "sentinel_session";
const CHALLENGE_TTL_MINS: i64 = 5;
/// Pending sign-in requests kept per address, and in total.
const MAX_CHALLENGES_PER_KEY: usize = 3;
const MAX_CHALLENGES: usize = 10_000;
pub const SESSION_DAYS: i64 = 30;

struct Challenge {
    pubkey: String,
    message: String,
    expires: DateTime<Utc>,
}

pub struct Auth {
    store: Arc<Store>,
    challenges: Mutex<HashMap<String, Challenge>>,
    domain: String,
    uri: String,
    secure_cookie: bool,
}

pub fn hash_token(token: &str) -> String {
    solana_sdk::hash::hashv(&[token.as_bytes()]).to_string()
}

fn random_hex() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

impl Auth {
    pub fn new(store: Arc<Store>, public_url: &str) -> Self {
        let domain = public_url
            .split("://")
            .nth(1)
            .unwrap_or(public_url)
            .split('/')
            .next()
            .unwrap_or("localhost")
            .to_string();
        Self {
            store,
            challenges: Mutex::new(HashMap::new()),
            domain,
            uri: public_url.trim_end_matches('/').to_string(),
            secure_cookie: public_url.starts_with("https://"),
        }
    }

    /// The exact text the wallet must sign.
    pub fn challenge(&self, pubkey: &str) -> Result<String> {
        Pubkey::from_str(pubkey).map_err(|_| anyhow!("not a valid Solana address"))?;
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let now = Utc::now();
        let expires = now + Duration::minutes(CHALLENGE_TTL_MINS);
        let message = format!(
            "{} wants you to sign in with your Solana account:\n{}\n\n\
             Sign in to Vortex Sentinel to manage your watchlist and alerts. \
             This does not send a transaction or cost anything.\n\n\
             URI: {}\nNonce: {}\nIssued At: {}\nExpiration Time: {}",
            self.domain,
            pubkey,
            self.uri,
            nonce,
            now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            expires.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        );
        let mut challenges = self.challenges.lock().unwrap();
        challenges.retain(|_, c| c.expires > now);
        // Keep only the newest few per address; a new request supersedes old ones.
        let mut mine: Vec<(String, DateTime<Utc>)> = challenges
            .iter()
            .filter(|(_, c)| c.pubkey == pubkey)
            .map(|(n, c)| (n.clone(), c.expires))
            .collect();
        if mine.len() >= MAX_CHALLENGES_PER_KEY {
            mine.sort_by_key(|(_, e)| *e);
            for (n, _) in mine.iter().take(mine.len() + 1 - MAX_CHALLENGES_PER_KEY) {
                challenges.remove(n);
            }
        }
        if challenges.len() >= MAX_CHALLENGES {
            bail!("Too many sign-in requests right now; try again in a few minutes");
        }
        challenges.insert(
            nonce,
            Challenge {
                pubkey: pubkey.to_string(),
                message: message.clone(),
                expires,
            },
        );
        Ok(message)
    }

    /// Checks the signed challenge and opens a session. Returns the session token.
    pub fn verify(&self, pubkey: &str, message: &str, signature: &str) -> Result<String> {
        let nonce = message
            .lines()
            .find_map(|l| l.strip_prefix("Nonce: "))
            .ok_or_else(|| anyhow!("message has no nonce"))?;
        // Single use: the challenge is consumed whether or not it verifies.
        let challenge = self
            .challenges
            .lock()
            .unwrap()
            .remove(nonce)
            .ok_or_else(|| anyhow!("sign-in request expired or already used; try again"))?;
        if challenge.expires < Utc::now() {
            bail!("sign-in request expired; try again");
        }
        if challenge.pubkey != pubkey || challenge.message != message {
            bail!("signed message doesn't match the sign-in request");
        }
        let pk = Pubkey::from_str(pubkey)?;
        let sig = Signature::from_str(signature).map_err(|_| anyhow!("malformed signature"))?;
        if !sig.verify(pk.as_ref(), message.as_bytes()) {
            bail!("signature does not match this wallet");
        }
        let token = random_hex();
        let expires = Utc::now() + Duration::days(SESSION_DAYS);
        self.store.create_session(&hash_token(&token), pubkey, expires)?;
        Ok(token)
    }

    /// A new API token for agents. The plaintext is returned once; only its hash is kept.
    pub fn create_api_token(&self, account: &str, name: &str, scope: &str) -> Result<(crate::model::ApiToken, String)> {
        let token = format!("snt_{}", random_hex());
        let record = self.store.create_api_token(&hash_token(&token), account, name, scope)?;
        Ok((record, token))
    }

    /// Who an API token belongs to, and what it may do: (token id, account, scope).
    pub fn api_token(&self, token: &str) -> Option<(i64, String, String)> {
        if !token.starts_with("snt_") {
            return None;
        }
        self.store.api_token_lookup(&hash_token(token)).ok().flatten()
    }

    pub fn account_for(&self, token: &str) -> Option<String> {
        self.store.session_account(&hash_token(token)).ok().flatten()
    }

    pub fn logout(&self, token: &str) {
        let _ = self.store.delete_session(&hash_token(token));
    }

    pub fn set_cookie(&self, token: &str) -> String {
        format!(
            "{COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{}",
            SESSION_DAYS * 86_400,
            if self.secure_cookie { "; Secure" } else { "" }
        )
    }

    pub fn clear_cookie(&self) -> String {
        format!("{COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0")
    }
}

pub fn session_token(parts: &Parts) -> Option<String> {
    parts
        .headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.to_string())
}

/// The signed-in account, if any. Never rejects.
pub struct Viewer(pub Option<String>);

/// The signed-in account; rejects with 401 otherwise.
pub struct Account(pub String);

impl FromRequestParts<Arc<crate::engine::Sentinel>> for Viewer {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &Arc<crate::engine::Sentinel>) -> Result<Self, Self::Rejection> {
        Ok(Viewer(session_token(parts).and_then(|t| state.auth.account_for(&t))))
    }
}

impl FromRequestParts<Arc<crate::engine::Sentinel>> for Account {
    type Rejection = (StatusCode, axum::Json<serde_json::Value>);

    async fn from_request_parts(parts: &mut Parts, state: &Arc<crate::engine::Sentinel>) -> Result<Self, Self::Rejection> {
        session_token(parts)
            .and_then(|t| state.auth.account_for(&t))
            .map(Account)
            .ok_or((
                StatusCode::UNAUTHORIZED,
                axum::Json(serde_json::json!({ "error": "Sign in with your wallet first" })),
            ))
    }
}
