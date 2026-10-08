//! HTTP + SSE API consumed by the dashboard.

use crate::alerts::{check_channel, check_webhook_url};
use crate::auth::{session_token, Account, Viewer};
use crate::engine::Sentinel;
use crate::model::{AlertRule, Channel, ChannelKind, Condition, DetectionConfig, IncidentStatus, Severity, SummaryPeriod, SummarySchedule, MASK};
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

pub struct ApiError(pub(crate) StatusCode, pub(crate) String);

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
        .route("/api/programs/{id}/posture", get(program_posture))
        .route("/api/programs/{id}/health", get(program_health))
        .route("/api/programs/{id}/idl", get(program_idl))
        .route("/api/programs/{id}/events", get(program_events))
        .route("/api/programs/{id}/suggestions", get(program_suggestions).post(apply_program_suggestions))
        .route("/api/programs/{id}/protect", post(protect_program))
        .route("/api/programs/{id}/mute", axum::routing::put(mute_program))
        .route("/api/programs/{id}/dependencies", get(program_dependencies))
        .route("/api/programs/{id}/vaults", get(program_vaults).put(set_program_vaults))
        .route("/api/programs/{id}/summary", get(program_summary))
        .route("/api/incidents", get(list_incidents))
        .route("/api/incidents/{id}", get(get_incident).patch(update_incident))
        .route("/api/incidents/{id}/timeline", get(incident_timeline))
        .route("/api/transactions/{signature}", get(get_transaction))
        .route("/api/tokens", get(list_tokens).post(create_token))
        .route("/api/tokens/{id}", axum::routing::delete(delete_token))
        .route("/api/incidents/{id}/report", get(incident_report))
        .route("/api/incidents/{id}/diagnosis", get(incident_diagnosis))
        .route("/api/summary-schedules", get(list_schedules).post(create_schedule))
        .route(
            "/api/summary-schedules/{id}",
            axum::routing::patch(update_schedule).delete(delete_schedule),
        )
        .route("/api/summary-schedules/{id}/send", post(send_schedule))
        .route("/api/rules", get(list_rules).post(create_rule))
        .route("/api/rules/{id}", axum::routing::patch(update_rule).delete(delete_rule))
        .route("/api/rules/{id}/test", post(test_rule))
        .route("/api/alerts", get(list_alerts))
        .route("/api/stream", get(stream))
        .route("/metrics", get(metrics))
        .route("/api/public/status/{id}", get(public_status))
        .route("/badge/{file}", get(badge))
        .merge(crate::mcp::router())
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
        "cluster": s.cluster,
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
    Ok(Json(add_program_inner(&s, &account, body.program_id, body.label)?))
}

pub(crate) fn add_program_inner(s: &Sentinel, account: &str, program_id: String, label: Option<String>) -> Result<Value, ApiError> {
    let program_id = program_id.trim().to_string();
    let watching = s.watching(account);
    if !s.limits.is_admin(account) && !watching.contains(&program_id) && watching.len() >= s.limits.max_watched {
        return Err(forbidden(&format!(
            "Your watchlist is full ({} programs). Remove one first.",
            s.limits.max_watched
        )));
    }
    let p = s.watch(account, program_id, label)?;
    Ok(json!(p))
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

async fn program_posture(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Value> {
    if !s.is_monitored(&id) {
        return Err(not_found("program"));
    }
    let posture = s
        .posture(&id)
        .await
        .map_err(|e| ApiError(StatusCode::BAD_GATEWAY, crate::redact::scrub(&e.to_string())))?;
    Ok(Json(json!(posture)))
}

/// Everything a public status page shows for one program. It uses only what the dashboard
/// already shows to anyone: health, incidents and a week of history.
async fn public_status(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Value> {
    if !s.is_monitored(&id) {
        return Err(not_found("program"));
    }
    let _ = s.posture(&id).await;
    let week = s.summary(&id, 7 * 24 * 3600)?;
    let health = s.health(&id).ok_or_else(|| not_found("program"))?;
    let labels = s.program_labels();
    let incidents: Vec<Value> = s
        .store
        .incidents(Some(&id), 60)?
        .into_iter()
        .filter(|i| !matches!(i.kind, crate::model::IncidentKind::LargeTransfer))
        .take(15)
        .map(|i| {
            json!({
                "id": i.id, "kind": i.kind.as_str(), "severity": i.severity, "status": i.status, "title": i.title,
                "summary": i.summary, "detected_at": i.detected_at, "resolved_at": i.resolved_at,
            })
        })
        .collect();
    let minutes = week.reliability.minutes_in_incident as f64;
    let covered_minutes = (week.period_secs as f64 / 60.0) * week.coverage.max(0.0);
    // The share of the time Sentinel watched that had no open reliability incident.
    let uptime = if covered_minutes > 0.0 { ((1.0 - minutes / covered_minutes) * 100.0).clamp(0.0, 100.0) } else { 100.0 };
    Ok(Json(json!({
        "program": { "id": id, "label": labels.get(&id) },
        "health": { "score": health.score, "status": health.status, "headline": health.headline },
        "uptime_percent": uptime,
        "uptime_days": 7,
        "coverage": week.coverage,
        "transactions_7d": week.activity.tx,
        "success_rate_7d": week.activity.success_rate,
        "incidents_7d": week.reliability.opened,
        "mean_time_to_resolve_secs": week.reliability.mttr_secs,
        "open_incidents": incidents.iter().filter(|i| i["status"] != "resolved").count(),
        "incidents": incidents,
        "generated_at": Utc::now(),
    })))
}

/// A small SVG showing a program's health, for a README or a docs page: `/badge/<program>.svg`.
async fn badge(State(s): State<AppState>, Path(file): Path<String>) -> Response {
    let id = file.trim_end_matches(".svg");
    let (value, color) = match s.health(id) {
        None => ("not monitored".to_string(), "#6f6e69"),
        Some(h) => match (h.score, h.status) {
            (Some(score), "healthy") => (format!("{score} healthy"), "#0b7f0b"),
            (Some(score), "degraded") => (format!("{score} degraded"), "#9a6400"),
            (Some(score), _) => (format!("{score} critical"), "#c22f2f"),
            (None, _) => ("learning".to_string(), "#6f6e69"),
        },
    };
    let label = "sentinel";
    // Width from the text, at about 6.5px a character plus padding.
    let lw = (label.len() as f64 * 6.5 + 14.0).round();
    let vw = (value.len() as f64 * 6.5 + 14.0).round();
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"20\" role=\"img\" aria-label=\"{label}: {value}\"><title>{label}: {value}</title>\
<linearGradient id=\"s\" x2=\"0\" y2=\"100%\"><stop offset=\"0\" stop-color=\"#bbb\" stop-opacity=\".1\"/><stop offset=\"1\" stop-opacity=\".1\"/></linearGradient>\
<clipPath id=\"r\"><rect width=\"{w}\" height=\"20\" rx=\"3\" fill=\"#fff\"/></clipPath>\
<g clip-path=\"url(#r)\"><rect width=\"{lw}\" height=\"20\" fill=\"#555\"/><rect x=\"{lw}\" width=\"{vw}\" height=\"20\" fill=\"{color}\"/><rect width=\"{w}\" height=\"20\" fill=\"url(#s)\"/></g>\
<g fill=\"#fff\" text-anchor=\"middle\" font-family=\"Verdana,Geneva,DejaVu Sans,sans-serif\" font-size=\"11\"><text x=\"{lx}\" y=\"14\">{label}</text><text x=\"{vx}\" y=\"14\">{value}</text></g></svg>",
        w = lw + vw,
        lx = lw / 2.0,
        vx = lw + vw / 2.0,
        value = value.replace('&', "&amp;").replace('<', "&lt;"),
    );
    (
        [(header::CONTENT_TYPE, "image/svg+xml"), (header::CACHE_CONTROL, "public, max-age=60")],
        svg,
    )
        .into_response()
}

/// Prometheus metrics. Set `SENTINEL_METRICS_TOKEN` to require `Authorization: Bearer <token>`.
async fn metrics(State(s): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    if let Ok(want) = std::env::var("SENTINEL_METRICS_TOKEN") {
        let given = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "));
        if !want.is_empty() && given != Some(want.as_str()) {
            return (StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE, "Bearer realm=\"sentinel-metrics\"")], "metrics need a bearer token").into_response();
        }
    }
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")], s.metrics_text()).into_response()
}

/// Names that usually mean someone is changing how a program works or taking money out.
pub const ADMIN_INSTRUCTIONS: &str = "set_*|update_*|withdraw*|pause*|unpause*|*authority*|*admin*|upgrade*|close*";

#[derive(Deserialize)]
struct ProtectInput {
    #[serde(default)]
    channels: Vec<Channel>,
    /// Reuse the channels of one of your rules instead, so no secret has to be sent again.
    channels_from_rule: Option<i64>,
}

async fn protect_program(
    State(s): State<AppState>,
    Account(account): Account,
    Path(id): Path<String>,
    Json(body): Json<ProtectInput>,
) -> ApiResult<Value> {
    Ok(Json(protect_inner(&s, &account, &id, body.channels, body.channels_from_rule).await?))
}

/// Creates the rules most teams want for a program, on the channels given. Rules that already
/// exist (same name, same program) are left alone, so running it twice changes nothing.
pub(crate) async fn protect_inner(
    s: &Sentinel,
    account: &str,
    program_id: &str,
    mut channels: Vec<Channel>,
    reuse: Option<i64>,
) -> Result<Value, ApiError> {
    if !s.is_watching(account, program_id) {
        return Err(forbidden("Watch the program first"));
    }
    if let Some(from) = reuse {
        let source = s
            .store
            .rules()?
            .into_iter()
            .find(|r| r.id == from && r.owner.as_deref() == Some(account))
            .ok_or_else(|| not_found("rule"))?;
        channels = source.targets();
    }
    if channels.is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "Choose where alerts should go: add a channel, or reuse one from an existing rule".into()));
    }
    let label = s.program_labels().get(program_id).cloned().unwrap_or_else(|| program_id.to_string());
    let existing: Vec<String> = s
        .store
        .rules()?
        .into_iter()
        .filter(|r| r.owner.as_deref() == Some(account))
        .map(|r| format!("{}|{}", r.program_id.as_deref().unwrap_or(""), r.name))
        .collect();

    let plan: Vec<(Option<&str>, String, Value, &str, bool)> = vec![
        (
            Some(program_id),
            format!("{label}: any incident of high severity or above"),
            json!({ "type": "incident", "kinds": [], "min_severity": "high" }),
            "high",
            false,
        ),
        (
            Some(program_id),
            format!("{label}: failure rate above 20%"),
            json!({ "type": "metric", "metric": "failure_rate", "op": ">", "value": 20, "window_secs": 60 }),
            "high",
            true,
        ),
        (
            Some(program_id),
            format!("{label}: admin instruction from a new wallet"),
            json!({ "type": "instruction", "name": ADMIN_INSTRUCTIONS, "filters": [], "match_mode": "all", "success_only": true, "first_seen_signer": true }),
            "critical",
            true,
        ),
        (
            Some(program_id),
            format!("{label}: health score below 60"),
            json!({ "type": "health", "below": 60 }),
            "high",
            true,
        ),
        (None, "Sentinel feed problems".to_string(), json!({ "type": "system", "kinds": [] }), "high", false),
    ];
    let mut created = Vec::new();
    let mut skipped = Vec::new();
    for (program, name, condition, severity, create_incident) in plan {
        if existing.contains(&format!("{}|{name}", program.unwrap_or(""))) {
            skipped.push(name);
            continue;
        }
        let input: RuleInput = serde_json::from_value(json!({
            "name": name, "program_id": program, "condition": condition, "severity": severity,
            "create_incident": create_incident, "channels": channels,
        }))
        .map_err(|e| ApiError(StatusCode::BAD_REQUEST, format!("Invalid channel: {e}")))?;
        created.push(create_rule_inner(s, account.to_string(), input).await?);
    }
    let vaults = s.vault_status(program_id).unwrap_or(Value::Null);
    let posture = s.posture(program_id).await.ok();
    let mut next = Vec::new();
    if vaults["vaults"].as_array().is_none_or(|v| v.is_empty()) {
        next.push("Name the program's vault or treasury accounts on its page so a drain opens an incident.".to_string());
    }
    if posture.as_ref().is_some_and(|p| p.authority_kind == "single_key") {
        next.push("One wallet can upgrade this program; move its upgrade authority to a multisig.".to_string());
    }
    if s.idls.cached(program_id).is_none() {
        next.push("No Anchor IDL was found, so instruction rules can match names but not arguments.".to_string());
    }
    Ok(json!({ "created": created, "already_had": skipped, "next_steps": next }))
}

#[derive(Deserialize)]
struct MuteInput {
    /// Minutes to hold notifications for, up to a week. 0 lifts it.
    minutes: u32,
    reason: Option<String>,
}

/// A maintenance window. Shared with everyone watching the program, so watchers may set it.
async fn mute_program(
    State(s): State<AppState>,
    Account(account): Account,
    Path(id): Path<String>,
    Json(body): Json<MuteInput>,
) -> ApiResult<Value> {
    Ok(Json(mute_inner(&s, &account, &id, body.minutes, body.reason)?))
}

pub(crate) fn mute_inner(s: &Sentinel, account: &str, id: &str, minutes: u32, reason: Option<String>) -> Result<Value, ApiError> {
    if !s.is_watching(account, id) && !s.limits.is_admin(account) {
        return Err(forbidden("Watch the program to mute it"));
    }
    Ok(json!(s.set_mute(id, minutes, reason)?))
}

/// The programs this one calls, with how often.
async fn program_dependencies(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Value> {
    Ok(Json(s.dependencies(&id).ok_or_else(|| not_found("program"))?))
}

/// The program's instructions with the accounts and arguments a rule can filter on.
async fn program_idl(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Value> {
    if !s.is_monitored(&id) {
        return Err(not_found("program"));
    }
    Ok(Json(match s.idls.get(&id).await {
        Some(idl) => json!({ "loaded": true, "name": idl.name, "instructions": idl.schema(), "events": idl.event_schema() }),
        None => json!({ "loaded": false, "name": null, "instructions": [], "events": [] }),
    }))
}

/// The latest events the program emitted, decoded with its IDL.
async fn program_events(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Value> {
    if !s.is_monitored(&id) {
        return Err(not_found("program"));
    }
    let idl = s.idls.get(&id).await;
    Ok(Json(json!({
        "idl_loaded": idl.is_some(),
        "declares_events": idl.as_ref().is_some_and(|i| i.has_events()),
        "events": s.recent_events(&id, 100),
    })))
}

/// Rules worth having for this program, read from its IDL.
async fn program_suggestions(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Value> {
    Ok(Json(suggestions_inner(&s, &id).await?))
}

pub(crate) async fn suggestions_inner(s: &Sentinel, id: &str) -> Result<Value, ApiError> {
    if !s.is_monitored(id) {
        return Err(not_found("program"));
    }
    Ok(match s.idls.get(id).await {
        Some(idl) => json!({ "idl_loaded": true, "suggestions": crate::suggest::from_idl(&idl) }),
        None => json!({ "idl_loaded": false, "suggestions": [] }),
    })
}

#[derive(Deserialize)]
struct ApplyInput {
    ids: Vec<String>,
    /// The number each suggestion that asks for one needs, by suggestion id.
    #[serde(default)]
    values: std::collections::HashMap<String, serde_json::Number>,
    #[serde(default)]
    channels: Vec<Channel>,
    channels_from_rule: Option<i64>,
}

async fn apply_program_suggestions(
    State(s): State<AppState>,
    Account(account): Account,
    Path(id): Path<String>,
    Json(body): Json<ApplyInput>,
) -> ApiResult<Value> {
    Ok(Json(apply_suggestions_inner(&s, &account, &id, body.ids, body.values, body.channels, body.channels_from_rule).await?))
}

/// Creates the chosen suggested rules on the channels given. A rule that already exists (same
/// name, same program) is left alone, so applying twice changes nothing.
pub(crate) async fn apply_suggestions_inner(
    s: &Sentinel,
    account: &str,
    program_id: &str,
    ids: Vec<String>,
    values: std::collections::HashMap<String, serde_json::Number>,
    mut channels: Vec<Channel>,
    reuse: Option<i64>,
) -> Result<Value, ApiError> {
    if !s.is_watching(account, program_id) {
        return Err(forbidden("Watch the program first"));
    }
    let idl = s.idls.get(program_id).await.ok_or_else(|| ApiError(StatusCode::BAD_REQUEST, "This program has no Anchor IDL to suggest rules from".into()))?;
    if let Some(from) = reuse {
        let source = s.store.rules()?.into_iter().find(|r| r.id == from && r.owner.as_deref() == Some(account)).ok_or_else(|| not_found("rule"))?;
        channels = source.targets();
    }
    if channels.is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "Choose where alerts should go: add a channel, or reuse one from an existing rule".into()));
    }
    let all = crate::suggest::from_idl(&idl);
    let label = s.program_labels().get(program_id).cloned().unwrap_or_else(|| program_id.to_string());
    let existing: Vec<String> = s.store.rules()?.into_iter().filter(|r| r.owner.as_deref() == Some(account)).map(|r| format!("{}|{}", r.program_id.as_deref().unwrap_or(""), r.name)).collect();
    let mut created = Vec::new();
    let mut skipped = Vec::new();
    for id in &ids {
        let Some(sg) = all.iter().find(|x| &x.id == id) else {
            return Err(ApiError(StatusCode::BAD_REQUEST, format!("No suggestion \"{id}\" for this program")));
        };
        let name = format!("{label}: {}", sg.title);
        if existing.contains(&format!("{program_id}|{name}")) {
            skipped.push(name);
            continue;
        }
        let mut condition = sg.condition.clone();
        if let Some(need) = &sg.needs_value {
            let value = values.get(id).filter(|v| v.as_f64().is_some_and(|f| f > 0.0)).ok_or_else(|| ApiError(StatusCode::BAD_REQUEST, format!("\"{}\" needs a value: {}", sg.title, need.label)))?;
            condition["filters"][0]["value"] = Value::Number(value.clone());
        }
        let input: RuleInput = serde_json::from_value(json!({
            "name": name, "program_id": program_id, "condition": condition, "severity": sg.severity,
            "create_incident": sg.create_incident, "channels": channels,
        }))
        .map_err(|e| ApiError(StatusCode::BAD_REQUEST, format!("Invalid rule: {e}")))?;
        created.push(create_rule_inner(s, account.to_string(), input).await?);
    }
    Ok(json!({ "created": created, "already_had": skipped }))
}

async fn program_health(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Value> {
    if !s.is_monitored(&id) {
        return Err(not_found("program"));
    }
    // Reads the upgrade authority too when it can; the check is left out of the score if not.
    let _ = s.posture(&id).await;
    let health = s.health(&id).ok_or_else(|| not_found("program"))?;
    Ok(Json(json!(health)))
}

async fn program_vaults(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Value> {
    Ok(Json(s.vault_status(&id).ok_or_else(|| not_found("program"))?))
}

#[derive(Deserialize)]
struct VaultsInput {
    vaults: Vec<String>,
}

/// Vaults are part of the program's shared detection settings, so anyone watching it may edit them.
async fn set_program_vaults(
    State(s): State<AppState>,
    Account(account): Account,
    Path(id): Path<String>,
    Json(body): Json<VaultsInput>,
) -> ApiResult<Value> {
    Ok(Json(set_vaults_inner(&s, &account, &id, &body.vaults)?))
}

pub(crate) fn set_vaults_inner(s: &Sentinel, account: &str, id: &str, requested: &[String]) -> Result<Value, ApiError> {
    if !s.is_watching(account, id) {
        return Err(forbidden("Watch the program to set its vaults"));
    }
    if requested.len() > 10 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "At most 10 vaults per program".into()));
    }
    let mut vaults = Vec::new();
    for v in requested {
        let v = v.trim();
        if v.parse::<solana_sdk::pubkey::Pubkey>().is_err() {
            return Err(ApiError(StatusCode::BAD_REQUEST, format!("{v} is not a valid account address")));
        }
        if !vaults.iter().any(|x| x == v) {
            vaults.push(v.to_string());
        }
    }
    s.set_vaults(id, vaults)?;
    s.vault_status(id).ok_or_else(|| not_found("program"))
}

#[derive(Deserialize)]
struct SummaryQuery {
    /// `1h`, `24h`, `7d`, `30d`, or any number of hours/days like `12h`.
    period: Option<String>,
}

/// "24h" -> 86400. Defaults to a day.
pub fn parse_period(text: Option<&str>) -> Option<i64> {
    let text = text.map(str::trim).filter(|t| !t.is_empty()).unwrap_or("24h");
    let (digits, unit) = text.split_at(text.find(|c: char| !c.is_ascii_digit())?);
    let n: i64 = digits.parse().ok().filter(|n| *n > 0)?;
    let secs = match unit {
        "h" => n * 3600,
        "d" => n * 86_400,
        _ => return None,
    };
    (3600..=30 * 86_400).contains(&secs).then_some(secs)
}

async fn program_summary(State(s): State<AppState>, Path(id): Path<String>, Query(q): Query<SummaryQuery>) -> ApiResult<Value> {
    if !s.is_monitored(&id) {
        return Err(not_found("program"));
    }
    let period = parse_period(q.period.as_deref())
        .ok_or_else(|| ApiError(StatusCode::BAD_REQUEST, "period must be 1h to 30d, like 24h or 7d".into()))?;
    let _ = s.posture(&id).await;
    Ok(Json(json!(s.summary(&id, period)?)))
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
    let found = s.explain_transaction(&sig).await?.ok_or_else(|| {
        ApiError(
            StatusCode::NOT_FOUND,
            "Transaction not found. It isn't in Sentinel's live window or an incident, and the \
             RPC doesn't have it (or SOLANA_RPC_URL isn't set)."
                .into(),
        )
    })?;
    Ok(Json(found))
}

async fn list_rules(State(s): State<AppState>, Account(account): Account) -> ApiResult<Value> {
    let mine: Vec<AlertRule> = s
        .store
        .rules()?
        .into_iter()
        .filter(|r| r.owner.as_deref() == Some(account.as_str()))
        .map(|r| r.masked())
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
pub(crate) struct RuleInput {
    name: String,
    program_id: Option<String>,
    condition: Condition,
    #[serde(default = "yes")]
    create_incident: bool,
    #[serde(default = "medium")]
    severity: Severity,
    webhook_url: Option<String>,
    #[serde(default)]
    channels: Vec<Channel>,
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
    if let Condition::Squads { multisig, .. } = &input.condition {
        if <solana_sdk::pubkey::Pubkey as std::str::FromStr>::from_str(multisig.trim()).is_err() {
            return Err(ApiError(StatusCode::BAD_REQUEST, "Enter the multisig's address".into()));
        }
        if input.program_id.as_deref().is_none_or(str::is_empty) {
            return Err(ApiError(StatusCode::BAD_REQUEST, "Pick the program this multisig controls".into()));
        }
    }
    if input.channels.len() > MAX_CHANNELS {
        return Err(ApiError(StatusCode::BAD_REQUEST, format!("A rule can have at most {MAX_CHANNELS} channels")));
    }
    if let Some(url) = input.webhook_url.as_deref().filter(|u| !u.is_empty() && !u.contains(MASK)) {
        check_webhook_url(url, s.allow_private_webhooks())
            .await
            .map_err(|e| ApiError(StatusCode::BAD_REQUEST, e.to_string()))?;
    }
    for channel in input.channels.iter().filter(|c| !c.is_masked()) {
        check_channel(channel, s.allow_private_webhooks())
            .await
            .map_err(|e| ApiError(StatusCode::BAD_REQUEST, format!("{}: {e}", channel.label())))?;
    }
    Ok(())
}

const MAX_CHANNELS: usize = 5;

/// Channels from a request, with masked secrets filled in from the rule they replace.
fn resolve_channels(input: &RuleInput, existing: Option<&AlertRule>) -> Result<Vec<Channel>, ApiError> {
    restore_channels(&input.channels, existing.map(|r| r.channels.as_slice()).unwrap_or_default())
}

fn restore_channels(channels: &[Channel], old: &[Channel]) -> Result<Vec<Channel>, ApiError> {
    channels
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mut c = c.clone();
            let kept = old.get(i).is_some_and(|o| c.restore_secret(o));
            if c.is_masked() && !kept {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    format!("{} secret is masked; enter it again", c.label()),
                ));
            }
            Ok(c)
        })
        .collect()
}

/// A legacy URL, unless the client sent a masked copy of the stored one.
fn resolve_webhook(input: &RuleInput, existing: Option<&AlertRule>) -> Option<String> {
    if !input.channels.is_empty() {
        return None;
    }
    let url = input.webhook_url.clone().filter(|u| !u.is_empty())?;
    if url.contains(MASK) {
        return existing.and_then(|r| r.webhook_url.clone());
    }
    Some(url)
}

async fn create_rule(State(s): State<AppState>, Account(account): Account, Json(input): Json<RuleInput>) -> ApiResult<Value> {
    Ok(Json(create_rule_inner(&s, account, input).await?))
}

/// Shared by the REST API and the MCP server so both enforce the same limits and checks.
pub(crate) async fn create_rule_inner(s: &Sentinel, account: String, input: RuleInput) -> Result<Value, ApiError> {
    let owned = s.store.rules()?.iter().filter(|r| r.owner.as_deref() == Some(account.as_str())).count();
    if !s.limits.is_admin(&account) && owned >= s.limits.max_rules {
        return Err(forbidden(&format!("You have the maximum of {} rules. Delete one first.", s.limits.max_rules)));
    }
    validate(s, &account, &input).await?;
    let channels = resolve_channels(&input, None)?;
    let webhook_url = resolve_webhook(&input, None);
    let rule = s.store.create_rule(AlertRule {
        id: 0,
        owner: Some(account),
        name: input.name.trim().to_string(),
        program_id: input.program_id.filter(|p| !p.is_empty()),
        condition: input.condition,
        create_incident: input.create_incident,
        severity: input.severity,
        webhook_url,
        channels,
        enabled: input.enabled,
        cooldown_secs: input.cooldown_secs,
        created_at: Utc::now(),
        last_fired_at: None,
    })?;
    s.reload_rules()?;
    Ok(json!(rule.masked()))
}

async fn update_rule(
    State(s): State<AppState>,
    Account(account): Account,
    Path(id): Path<i64>,
    Json(input): Json<RuleInput>,
) -> ApiResult<Value> {
    let existing = owned_rule(&s, &account, id)?;
    validate(&s, &account, &input).await?;
    let channels = resolve_channels(&input, Some(&existing))?;
    let webhook_url = resolve_webhook(&input, Some(&existing));
    let rule = AlertRule {
        name: input.name.trim().to_string(),
        program_id: input.program_id.filter(|p| !p.is_empty()),
        condition: input.condition,
        create_incident: input.create_incident,
        severity: input.severity,
        webhook_url,
        channels,
        enabled: input.enabled,
        cooldown_secs: input.cooldown_secs,
        ..existing
    };
    s.store.update_rule(&rule)?;
    s.reload_rules()?;
    Ok(Json(json!(rule.masked())))
}

// ------------------------------------------------------------ API tokens

/// Agent tokens one account can have.
const MAX_TOKENS: usize = 10;

#[derive(Deserialize)]
struct TokenInput {
    name: String,
    /// "read" (the default) or "write".
    scope: Option<String>,
}

async fn list_tokens(State(s): State<AppState>, Account(account): Account) -> ApiResult<Value> {
    Ok(Json(json!(s.store.api_tokens(&account)?)))
}

/// Creates a token for agents (the MCP server). The secret is shown once, here.
async fn create_token(State(s): State<AppState>, Account(account): Account, Json(input): Json<TokenInput>) -> ApiResult<Value> {
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 60 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "Name the token (up to 60 characters)".into()));
    }
    let scope = input.scope.as_deref().unwrap_or("read");
    if !matches!(scope, "read" | "write") {
        return Err(ApiError(StatusCode::BAD_REQUEST, "scope must be \"read\" or \"write\"".into()));
    }
    if s.store.api_tokens(&account)?.len() >= MAX_TOKENS {
        return Err(forbidden(&format!("You have the maximum of {MAX_TOKENS} tokens. Revoke one first.")));
    }
    let (record, secret) = s.auth.create_api_token(&account, name, scope)?;
    Ok(Json(json!({ "token": record, "secret": secret })))
}

async fn delete_token(State(s): State<AppState>, Account(account): Account, Path(id): Path<i64>) -> ApiResult<Value> {
    if !s.store.delete_api_token(&account, id)? {
        return Err(not_found("token"));
    }
    Ok(Json(json!({ "ok": true })))
}

/// A post-mortem for the incident, as markdown.
async fn incident_report(State(s): State<AppState>, Path(id): Path<i64>) -> Result<Response, ApiError> {
    let markdown = s.incident_report(id)?.ok_or_else(|| not_found("incident"))?;
    Ok(([(header::CONTENT_TYPE, "text/markdown; charset=utf-8")], markdown).into_response())
}

async fn incident_diagnosis(State(s): State<AppState>, Path(id): Path<i64>) -> ApiResult<Value> {
    Ok(Json(json!(s.diagnose_incident(id)?.ok_or_else(|| not_found("incident"))?)))
}

// ------------------------------------------------------------ summary schedules

/// Scheduled reports one account can have.
const MAX_SCHEDULES: usize = 10;

#[derive(Deserialize)]
struct ScheduleInput {
    program_id: String,
    period: SummaryPeriod,
    #[serde(default = "nine")]
    hour_utc: u8,
    channels: Vec<Channel>,
    #[serde(default = "yes")]
    enabled: bool,
}

fn nine() -> u8 {
    9
}

async fn validate_schedule(s: &Sentinel, account: &str, input: &ScheduleInput) -> Result<(), ApiError> {
    if !s.is_watching(account, &input.program_id) {
        return Err(forbidden("Summaries can only be scheduled for programs on your watchlist"));
    }
    if input.hour_utc > 23 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "hour_utc must be 0 to 23".into()));
    }
    if input.channels.is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "Add at least one channel to send the summary to".into()));
    }
    if input.channels.len() > MAX_CHANNELS {
        return Err(ApiError(StatusCode::BAD_REQUEST, format!("A summary can have at most {MAX_CHANNELS} channels")));
    }
    if input.channels.iter().any(|c| matches!(c.kind, ChannelKind::Pagerduty { .. })) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "PagerDuty is for pages; send summaries to Slack, Telegram, Discord or a webhook".into()));
    }
    for channel in input.channels.iter().filter(|c| !c.is_masked()) {
        check_channel(channel, s.allow_private_webhooks())
            .await
            .map_err(|e| ApiError(StatusCode::BAD_REQUEST, format!("{}: {e}", channel.label())))?;
    }
    Ok(())
}

fn owned_schedule(s: &Sentinel, account: &str, id: i64) -> Result<SummarySchedule, ApiError> {
    s.store
        .schedules()?
        .into_iter()
        .find(|x| x.id == id && x.owner == account)
        .ok_or_else(|| not_found("summary schedule"))
}

async fn list_schedules(State(s): State<AppState>, Account(account): Account) -> ApiResult<Value> {
    let mine: Vec<SummarySchedule> = s.store.schedules()?.into_iter().filter(|x| x.owner == account).map(|x| x.masked()).collect();
    Ok(Json(json!(mine)))
}

async fn create_schedule(State(s): State<AppState>, Account(account): Account, Json(input): Json<ScheduleInput>) -> ApiResult<Value> {
    let owned = s.store.schedules()?.iter().filter(|x| x.owner == account).count();
    if !s.limits.is_admin(&account) && owned >= MAX_SCHEDULES {
        return Err(forbidden(&format!("You have the maximum of {MAX_SCHEDULES} scheduled summaries. Delete one first.")));
    }
    validate_schedule(&s, &account, &input).await?;
    let channels = restore_channels(&input.channels, &[])?;
    let schedule = s.store.create_schedule(SummarySchedule {
        id: 0,
        owner: account,
        program_id: input.program_id,
        period: input.period,
        hour_utc: input.hour_utc,
        channels,
        enabled: input.enabled,
        created_at: Utc::now(),
        last_sent_at: None,
    })?;
    Ok(Json(json!(schedule.masked())))
}

async fn update_schedule(
    State(s): State<AppState>,
    Account(account): Account,
    Path(id): Path<i64>,
    Json(input): Json<ScheduleInput>,
) -> ApiResult<Value> {
    let existing = owned_schedule(&s, &account, id)?;
    validate_schedule(&s, &account, &input).await?;
    let channels = restore_channels(&input.channels, &existing.channels)?;
    let schedule = SummarySchedule {
        program_id: input.program_id,
        period: input.period,
        hour_utc: input.hour_utc,
        channels,
        enabled: input.enabled,
        ..existing
    };
    s.store.update_schedule(&schedule)?;
    Ok(Json(json!(schedule.masked())))
}

async fn delete_schedule(State(s): State<AppState>, Account(account): Account, Path(id): Path<i64>) -> ApiResult<Value> {
    owned_schedule(&s, &account, id)?;
    s.store.delete_schedule(id)?;
    Ok(Json(json!({ "ok": true })))
}

/// Sends the report now without waiting for its hour.
async fn send_schedule(State(s): State<AppState>, Account(account): Account, Path(id): Path<i64>) -> ApiResult<Value> {
    let schedule = owned_schedule(&s, &account, id)?;
    if !s.is_monitored(&schedule.program_id) {
        return Err(not_found("program"));
    }
    s.send_summary(&schedule)?;
    Ok(Json(json!({ "queued": true })))
}

async fn delete_rule(State(s): State<AppState>, Account(account): Account, Path(id): Path<i64>) -> ApiResult<Value> {
    owned_rule(&s, &account, id)?;
    s.store.delete_rule(id)?;
    s.reload_rules()?;
    Ok(Json(json!({ "ok": true })))
}

async fn test_rule(State(s): State<AppState>, Account(account): Account, Path(id): Path<i64>) -> ApiResult<Value> {
    let rule = owned_rule(&s, &account, id)?;
    if !rule.has_targets() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "Rule has no channel to deliver to".into()));
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
