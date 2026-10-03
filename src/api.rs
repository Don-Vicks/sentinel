//! HTTP + SSE API consumed by the dashboard.

use crate::alerts::check_webhook_url;
use crate::auth::{session_token, Account, Viewer};
use crate::engine::Sentinel;
use crate::model::{AlertRule, Condition, DetectionConfig, IncidentStatus, Severity};
use crate::trace;
use axum::extract::{Path, Query, State};
use axum::http::{header, request::Parts, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use futures_util::{Stream, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::sync::Arc;
use tokio_stream::wrappers::BroadcastStream;

type AppState = Arc<Sentinel>;

pub struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": crate::redact::scrub(&self.1) }))).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        // Client errors embed the request URL, which carries the Solami key.
        ApiError(StatusCode::BAD_REQUEST, crate::redact::scrub(&format!("{e:#}")))
    }
}

fn not_found(what: &str) -> ApiError {
    ApiError(StatusCode::NOT_FOUND, format!("{what} not found"))
}

type ApiResult<T> = Result<Json<T>, ApiError>;

pub fn router(sentinel: AppState) -> Router {
    Router::new()
        .route("/api/auth/challenge", post(auth_challenge))
        .route("/api/auth/verify", post(auth_verify))
        .route("/api/auth/logout", post(auth_logout))
        .route("/api/auth/me", get(auth_me))
        .route("/api/status", get(status))
        .route("/api/solami", get(solami))
        .route("/api/catalog", get(catalog))
        .route("/api/resolve", post(resolve))
        .route("/api/programs", get(list_programs).post(add_program))
        .route(
            "/api/programs/{id}",
            get(get_program).patch(update_program).delete(remove_program),
        )
        .route("/api/programs/{id}/transactions", get(program_transactions))
        .route("/api/incidents", get(list_incidents))
        .route("/api/incidents/{id}", get(get_incident).patch(update_incident))
        .route("/api/incidents/{id}/timeline", get(incident_timeline))
        .route("/api/transactions/{signature}", get(get_transaction))
        .route("/api/rules", get(list_rules).post(create_rule))
        .route("/api/rules/{id}", axum::routing::patch(update_rule).delete(delete_rule))
        .route("/api/rules/{id}/test", post(test_rule))
        .route("/api/alerts", get(list_alerts))
        .route("/api/stream", get(stream))
        .layer(axum::middleware::from_fn_with_state(sentinel.clone(), crate::limits::rate_limit))
        .with_state(sentinel)
}

// ------------------------------------------------------------------ auth

#[derive(Deserialize)]
struct ChallengeInput {
    pubkey: String,
}

async fn auth_challenge(State(s): State<AppState>, Json(body): Json<ChallengeInput>) -> ApiResult<Value> {
    Ok(Json(json!({ "message": s.auth.challenge(body.pubkey.trim())? })))
}

#[derive(Deserialize)]
struct VerifyInput {
    pubkey: String,
    message: String,
    /// Base58 ed25519 signature over `message`.
    signature: String,
}

async fn auth_verify(State(s): State<AppState>, Json(body): Json<VerifyInput>) -> Result<Response, ApiError> {
    let token = s
        .auth
        .verify(&body.pubkey, &body.message, &body.signature)
        .map_err(|e| ApiError(StatusCode::UNAUTHORIZED, e.to_string()))?;
    let me = json!({ "account": body.pubkey, "watching": s.watching(&body.pubkey) });
    Ok(([(header::SET_COOKIE, s.auth.set_cookie(&token))], Json(me)).into_response())
}

async fn auth_logout(State(s): State<AppState>, parts: Parts) -> Response {
    if let Some(token) = session_token(&parts) {
        s.auth.logout(&token);
    }
    ([(header::SET_COOKIE, s.auth.clear_cookie())], Json(json!({ "ok": true }))).into_response()
}

async fn auth_me(State(s): State<AppState>, Viewer(account): Viewer) -> ApiResult<Value> {
    let watching = account.as_deref().map(|a| s.watching(a)).unwrap_or_default();
    Ok(Json(json!({ "account": account, "watching": watching })))
}

fn forbidden(msg: &str) -> ApiError {
    ApiError(StatusCode::FORBIDDEN, msg.into())
}

// --------------------------------------------------------------- programs

async fn status(State(s): State<AppState>, Viewer(account): Viewer) -> ApiResult<Value> {
    Ok(Json(json!({
        "account": account,
        "watching": account.as_deref().map(|a| s.watching(a)).unwrap_or_default(),
        "stream": s.stream_health(),
        "pricing": s.prices.status(),
        "solami": s.solami_status(),
        "programs": s.programs(),
        "series": s.programs_series(300),
        "now": Utc::now(),
    })))
}

/// Well-known programs, plus a balanced set for demos.
async fn catalog() -> Json<Value> {
    Json(json!({
        "programs": crate::catalog::CATALOG,
        "showcase": crate::catalog::showcase().iter().map(|e| e.id).collect::<Vec<_>>(),
    }))
}

/// What each Solami product is doing for this instance.
async fn solami(State(s): State<AppState>) -> ApiResult<Value> {
    Ok(Json(json!(s.solami_status())))
}

async fn list_programs(State(s): State<AppState>) -> ApiResult<Value> {
    Ok(Json(json!(s.programs())))
}

#[derive(Deserialize)]
struct AddProgram {
    program_id: String,
    label: Option<String>,
}

async fn add_program(
    State(s): State<AppState>,
    Account(account): Account,
    Json(body): Json<AddProgram>,
) -> ApiResult<Value> {
    let program_id = body.program_id.trim().to_string();
    let watching = s.watching(&account);
    if !s.limits.is_admin(&account) && !watching.contains(&program_id) && watching.len() >= s.limits.max_watched {
        return Err(forbidden(&format!(
            "Your watchlist is full ({} programs). Remove one first.",
            s.limits.max_watched
        )));
    }
    let p = s.watch(&account, program_id, body.label)?;
    Ok(Json(json!(p)))
}

#[derive(Deserialize)]
struct ResolveQuery {
    query: String,
}

/// Works out what was pasted (program, authority, signature, explorer link)
/// and which programs it points at. Public data only; no sign-in needed.
async fn resolve(State(s): State<AppState>, Json(body): Json<ResolveQuery>) -> ApiResult<Value> {
    use crate::resolve::{parse_query, resolve_address, resolve_transaction, Query};
    let q = parse_query(&body.query)?;
    let lookup = async {
        match q {
            Query::Signature(sig) => {
                let tx = s
                    .transaction(&sig)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("Transaction not found on mainnet"))?;
                Ok(resolve_transaction(&sig, &tx))
            }
            Query::Address(addr) => {
                let rpc = s.rpc.as_ref().ok_or_else(|| anyhow::anyhow!("Lookups need an RPC endpoint configured"))?;
                resolve_address(rpc, &addr).await
            }
        }
    };
    let found = tokio::time::timeout(std::time::Duration::from_secs(20), lookup)
        .await
        .map_err(|_| ApiError(StatusCode::GATEWAY_TIMEOUT, "The lookup timed out; try again".into()))??;
    Ok(Json(json!(found)))
}

async fn get_program(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Value> {
    let mut detail = s.program_detail(&id).ok_or_else(|| not_found("program"))?;
    detail["incidents"] = json!(s.store.incidents(Some(&id), 20)?);
    Ok(Json(detail))
}

#[derive(Deserialize)]
struct UpdateProgram {
    label: Option<String>,
    detection: Option<DetectionConfig>,
}

async fn update_program(
    State(s): State<AppState>,
    Account(account): Account,
    Path(id): Path<String>,
    Json(body): Json<UpdateProgram>,
) -> ApiResult<Value> {
    // Detection settings are shared by everyone watching the program.
    if !s.limits.is_admin(&account) {
        return Err(forbidden("Only the operator can change shared detection settings"));
    }
    Ok(Json(json!(s.update_program(&id, body.label, body.detection)?)))
}

/// Removes the program from the caller's watchlist.
async fn remove_program(State(s): State<AppState>, Account(account): Account, Path(id): Path<String>) -> ApiResult<Value> {
    s.unwatch(&account, &id)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct TxQuery {
    failed: Option<bool>,
    limit: Option<usize>,
}

async fn program_transactions(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<TxQuery>,
) -> ApiResult<Value> {
    Ok(Json(json!(s.recent_transactions(
        &id,
        q.failed.unwrap_or(false),
        q.limit.unwrap_or(100).min(500)
    ))))
}

#[derive(Deserialize)]
struct IncidentQuery {
    program: Option<String>,
    limit: Option<i64>,
}

async fn list_incidents(State(s): State<AppState>, Query(q): Query<IncidentQuery>) -> ApiResult<Value> {
    Ok(Json(json!(s
        .store
        .incidents(q.program.as_deref(), q.limit.unwrap_or(100).min(500))?)))
}

async fn get_incident(State(s): State<AppState>, Path(id): Path<i64>) -> ApiResult<Value> {
    let incident = s.store.incident(id)?.ok_or_else(|| not_found("incident"))?;
    let transactions = s.store.incident_transactions(id, 300)?;
    let labels = s.program_labels();
    Ok(Json(json!({
        "incident": incident,
        "program_label": labels.get(&incident.program_id),
        "transactions": transactions,
    })))
}

async fn incident_timeline(State(s): State<AppState>, Path(id): Path<i64>) -> ApiResult<Value> {
    let incident = s.store.incident(id)?.ok_or_else(|| not_found("incident"))?;
    Ok(Json(s.incident_timeline(&incident).unwrap_or(Value::Null)))
}

#[derive(Deserialize)]
struct UpdateIncident {
    status: IncidentStatus,
}

async fn update_incident(
    State(s): State<AppState>,
    Account(account): Account,
    Path(id): Path<i64>,
    Json(body): Json<UpdateIncident>,
) -> ApiResult<Value> {
    let incident = s.store.incident(id)?.ok_or_else(|| not_found("incident"))?;
    if !s.limits.is_admin(&account) && !s.is_watching(&account, &incident.program_id) {
        return Err(forbidden("Watch this program to update its incidents"));
    }
    Ok(Json(json!(s.set_incident_status(id, body.status)?)))
}

async fn get_transaction(State(s): State<AppState>, Path(sig): Path<String>) -> ApiResult<Value> {
    let tx = s.transaction(&sig).await?.ok_or_else(|| {
        ApiError(
            StatusCode::NOT_FOUND,
            "Transaction not found. It isn't in Sentinel's live window or an incident, and the \
             RPC doesn't have it (or SOLANA_RPC_URL isn't set)."
                .into(),
        )
    })?;
    let labels = s.program_labels();
    let (mut trace, landing, tip) = tokio::join!(
        trace::build(&tx, &labels, s.rpc.as_deref(), &s.owners, &s.prices, Some(&s.idls)),
        s.beam.landing(&sig),
        s.beam.tip_in(&tx),
    );
    if let Some(line) = crate::beam::describe(landing.as_ref(), tip.as_ref()) {
        trace.narrative.push(line);
    }
    let programs: Vec<&String> = labels.keys().filter(|p| tx.touches(p)).collect();
    Ok(Json(json!({
        "transaction": tx,
        "trace": trace,
        "beam": { "landing": landing, "tip": tip },
        "monitored_programs": programs,
        "program_labels": labels,
        "incidents": s.store.incidents_for_transaction(&sig)?,
    })))
}

async fn list_rules(State(s): State<AppState>, Account(account): Account) -> ApiResult<Value> {
    let mine: Vec<AlertRule> = s
        .store
        .rules()?
        .into_iter()
        .filter(|r| r.owner.as_deref() == Some(account.as_str()))
        .collect();
    Ok(Json(json!(mine)))
}

fn owned_rule(s: &Sentinel, account: &str, id: i64) -> Result<AlertRule, ApiError> {
    let rule = s
        .store
        .rules()?
        .into_iter()
        .find(|r| r.id == id)
        .ok_or_else(|| not_found("rule"))?;
    if rule.owner.as_deref() != Some(account) {
        return Err(not_found("rule"));
    }
    Ok(rule)
}

#[derive(Deserialize)]
struct RuleInput {
    name: String,
    program_id: Option<String>,
    condition: Condition,
    #[serde(default = "yes")]
    create_incident: bool,
    #[serde(default = "medium")]
    severity: Severity,
    webhook_url: Option<String>,
    #[serde(default = "yes")]
    enabled: bool,
    #[serde(default = "default_cooldown")]
    cooldown_secs: u32,
}

fn yes() -> bool {
    true
}
fn medium() -> Severity {
    Severity::Medium
}
fn default_cooldown() -> u32 {
    300
}

async fn validate(s: &Sentinel, account: &str, input: &RuleInput) -> Result<(), ApiError> {
    if input.name.trim().is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "Name the rule".into()));
    }
    if let Some(pid) = input.program_id.as_deref().filter(|p| !p.is_empty()) {
        if !s.is_watching(account, pid) {
            return Err(forbidden("Rules can only target programs on your watchlist"));
        }
    }
    if let Some(url) = input.webhook_url.as_deref().filter(|u| !u.is_empty()) {
        check_webhook_url(url, s.allow_private_webhooks())
            .await
            .map_err(|e| ApiError(StatusCode::BAD_REQUEST, e.to_string()))?;
    }
    Ok(())
}

async fn create_rule(State(s): State<AppState>, Account(account): Account, Json(input): Json<RuleInput>) -> ApiResult<Value> {
    let owned = s.store.rules()?.iter().filter(|r| r.owner.as_deref() == Some(account.as_str())).count();
    if !s.limits.is_admin(&account) && owned >= s.limits.max_rules {
        return Err(forbidden(&format!("You have the maximum of {} rules. Delete one first.", s.limits.max_rules)));
    }
    validate(&s, &account, &input).await?;
    let rule = s.store.create_rule(AlertRule {
        id: 0,
        owner: Some(account),
        name: input.name.trim().to_string(),
        program_id: input.program_id.filter(|p| !p.is_empty()),
        condition: input.condition,
        create_incident: input.create_incident,
        severity: input.severity,
        webhook_url: input.webhook_url.filter(|u| !u.is_empty()),
        enabled: input.enabled,
        cooldown_secs: input.cooldown_secs,
        created_at: Utc::now(),
        last_fired_at: None,
    })?;
    s.reload_rules()?;
    Ok(Json(json!(rule)))
}

async fn update_rule(
    State(s): State<AppState>,
    Account(account): Account,
    Path(id): Path<i64>,
    Json(input): Json<RuleInput>,
) -> ApiResult<Value> {
    let existing = owned_rule(&s, &account, id)?;
    validate(&s, &account, &input).await?;
    let rule = AlertRule {
        name: input.name.trim().to_string(),
        program_id: input.program_id.filter(|p| !p.is_empty()),
        condition: input.condition,
        create_incident: input.create_incident,
        severity: input.severity,
        webhook_url: input.webhook_url.filter(|u| !u.is_empty()),
        enabled: input.enabled,
        cooldown_secs: input.cooldown_secs,
        ..existing
    };
    s.store.update_rule(&rule)?;
    s.reload_rules()?;
    Ok(Json(json!(rule)))
}

async fn delete_rule(State(s): State<AppState>, Account(account): Account, Path(id): Path<i64>) -> ApiResult<Value> {
    owned_rule(&s, &account, id)?;
    s.store.delete_rule(id)?;
    s.reload_rules()?;
    Ok(Json(json!({ "ok": true })))
}

async fn test_rule(State(s): State<AppState>, Account(account): Account, Path(id): Path<i64>) -> ApiResult<Value> {
    let rule = owned_rule(&s, &account, id)?;
    if rule.webhook_url.is_none() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "Rule has no webhook URL".into()));
    }
    s.test_rule(rule);
    Ok(Json(json!({ "queued": true })))
}

async fn list_alerts(State(s): State<AppState>, Account(account): Account) -> ApiResult<Value> {
    Ok(Json(json!(s.store.executions_for(&account, 100)?)))
}

#[derive(Deserialize)]
struct StreamQuery {
    program: Option<String>,
}

async fn stream(
    State(s): State<AppState>,
    Viewer(account): Viewer,
    Query(q): Query<StreamQuery>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let hello = Event::default()
        .event("stream")
        .json_data(json!({ "type": "stream", "health": s.stream_health() }))
        .ok();
    let events = BroadcastStream::new(s.live.subscribe()).filter_map(move |msg| {
        let program = q.program.clone();
        let account = account.clone();
        async move {
            let ev = msg.ok()?;
            // Alert deliveries are private to the rule's owner.
            if let crate::live::LiveEvent::Alert { execution } = &*ev {
                if execution.owner.is_some() && execution.owner != account {
                    return None;
                }
            }
            if let (Some(want), Some(have)) = (program.as_deref(), ev.program_id()) {
                if want != have {
                    return None;
                }
            }
            Event::default().event(ev.name()).json_data(&*ev).ok().map(Ok)
        }
    });
    let stream = futures_util::stream::iter(hello.map(Ok)).chain(events);
    Sse::new(stream).keep_alive(KeepAlive::default())
}
