//! HTTP + SSE API consumed by the dashboard.

use crate::engine::Sentinel;
use crate::model::{AlertRule, Condition, DetectionConfig, IncidentStatus, Severity};
use crate::trace;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
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
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError(StatusCode::BAD_REQUEST, e.to_string())
    }
}

fn not_found(what: &str) -> ApiError {
    ApiError(StatusCode::NOT_FOUND, format!("{what} not found"))
}

type ApiResult<T> = Result<Json<T>, ApiError>;

pub fn router(sentinel: AppState) -> Router {
    Router::new()
        .route("/api/status", get(status))
        .route("/api/programs", get(list_programs).post(add_program))
        .route(
            "/api/programs/{id}",
            get(get_program).patch(update_program).delete(remove_program),
        )
        .route("/api/programs/{id}/transactions", get(program_transactions))
        .route("/api/incidents", get(list_incidents))
        .route("/api/incidents/{id}", get(get_incident).patch(update_incident))
        .route("/api/transactions/{signature}", get(get_transaction))
        .route("/api/rules", get(list_rules).post(create_rule))
        .route("/api/rules/{id}", axum::routing::patch(update_rule).delete(delete_rule))
        .route("/api/rules/{id}/test", post(test_rule))
        .route("/api/alerts", get(list_alerts))
        .route("/api/stream", get(stream))
        .with_state(sentinel)
}

async fn status(State(s): State<AppState>) -> ApiResult<Value> {
    Ok(Json(json!({
        "stream": s.stream_health(),
        "programs": s.programs(),
        "now": Utc::now(),
    })))
}

async fn list_programs(State(s): State<AppState>) -> ApiResult<Value> {
    Ok(Json(json!(s.programs())))
}

#[derive(Deserialize)]
struct AddProgram {
    program_id: String,
    label: Option<String>,
}

async fn add_program(State(s): State<AppState>, Json(body): Json<AddProgram>) -> ApiResult<Value> {
    let p = s.add_program(body.program_id.trim().to_string(), body.label)?;
    Ok(Json(json!(p)))
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
    Path(id): Path<String>,
    Json(body): Json<UpdateProgram>,
) -> ApiResult<Value> {
    Ok(Json(json!(s.update_program(&id, body.label, body.detection)?)))
}

async fn remove_program(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Value> {
    s.remove_program(&id)?;
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

#[derive(Deserialize)]
struct UpdateIncident {
    status: IncidentStatus,
}

async fn update_incident(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<UpdateIncident>,
) -> ApiResult<Value> {
    Ok(Json(json!(s.set_incident_status(id, body.status)?)))
}

async fn get_transaction(State(s): State<AppState>, Path(sig): Path<String>) -> ApiResult<Value> {
    let tx = s.transaction(&sig).ok_or_else(|| {
        ApiError(
            StatusCode::NOT_FOUND,
            "Transaction is not in Sentinel's window. Only recent transactions and those linked \
             to incidents are kept."
                .into(),
        )
    })?;
    let labels = s.program_labels();
    let trace = trace::build(&tx, &labels, s.rpc.as_deref(), &s.owners).await;
    let programs: Vec<&String> = labels.keys().filter(|p| tx.touches(p)).collect();
    Ok(Json(json!({
        "transaction": tx,
        "trace": trace,
        "monitored_programs": programs,
        "program_labels": labels,
        "incidents": s.store.incidents_for_transaction(&sig)?,
    })))
}

async fn list_rules(State(s): State<AppState>) -> ApiResult<Value> {
    Ok(Json(json!(s.store.rules()?)))
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

fn validate(input: &RuleInput) -> Result<(), ApiError> {
    if input.name.trim().is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "Name the rule".into()));
    }
    if let Some(url) = input.webhook_url.as_deref().filter(|u| !u.is_empty()) {
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(ApiError(StatusCode::BAD_REQUEST, "Webhook URL must start with http:// or https://".into()));
        }
    }
    Ok(())
}

async fn create_rule(State(s): State<AppState>, Json(input): Json<RuleInput>) -> ApiResult<Value> {
    validate(&input)?;
    let rule = s.store.create_rule(AlertRule {
        id: 0,
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
    Path(id): Path<i64>,
    Json(input): Json<RuleInput>,
) -> ApiResult<Value> {
    validate(&input)?;
    let existing = s
        .store
        .rules()?
        .into_iter()
        .find(|r| r.id == id)
        .ok_or_else(|| not_found("rule"))?;
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

async fn delete_rule(State(s): State<AppState>, Path(id): Path<i64>) -> ApiResult<Value> {
    s.store.delete_rule(id)?;
    s.reload_rules()?;
    Ok(Json(json!({ "ok": true })))
}

async fn test_rule(State(s): State<AppState>, Path(id): Path<i64>) -> ApiResult<Value> {
    let rule = s
        .store
        .rules()?
        .into_iter()
        .find(|r| r.id == id)
        .ok_or_else(|| not_found("rule"))?;
    if rule.webhook_url.is_none() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "Rule has no webhook URL".into()));
    }
    s.test_rule(rule);
    Ok(Json(json!({ "queued": true })))
}

async fn list_alerts(State(s): State<AppState>) -> ApiResult<Value> {
    Ok(Json(json!(s.store.executions(100)?)))
}

#[derive(Deserialize)]
struct StreamQuery {
    program: Option<String>,
}

async fn stream(
    State(s): State<AppState>,
    Query(q): Query<StreamQuery>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let hello = Event::default()
        .event("stream")
        .json_data(json!({ "type": "stream", "health": s.stream_health() }))
        .ok();
    let events = BroadcastStream::new(s.live.subscribe()).filter_map(move |msg| {
        let program = q.program.clone();
        async move {
            let ev = msg.ok()?;
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
