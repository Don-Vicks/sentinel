//! A Model Context Protocol server, so an agent (Claude, Cursor, ...) can watch
//! programs, read health and incidents, diagnose what broke and set up
//! alerts. It speaks JSON-RPC over HTTP at `/mcp`, authenticated with an API
//! token created on the Alerts page. Tokens are read-only unless created with
//! write scope, and every tool goes through the same checks as the REST API.

use crate::api::{self, ApiError, RuleInput};
use crate::engine::Sentinel;
use crate::model::IncidentStatus;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Map, Value};
use std::sync::Arc;

const SUPPORTED: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Vortex Sentinel watches Solana programs on mainnet and turns their transactions into incidents. \
Start with list_programs, then get_health for a program, list_incidents for what is wrong, and diagnose_incident for why. \
explain_transaction shows any signature's value flow and call tree. get_summary reports what a program did over a period. \
Everything returned is recorded data; say so when you are inferring beyond it. Write tools exist only on write-scoped tokens.";

pub fn router() -> Router<Arc<Sentinel>> {
    Router::new().route("/mcp", post(handle).get(not_allowed))
}

async fn not_allowed() -> Response {
    (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, "POST")], "Send JSON-RPC requests with POST").into_response()
}

struct Ctx {
    account: String,
    token_id: i64,
    write: bool,
}

fn unauthorized(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer realm=\"sentinel\"")],
        Json(json!({ "error": message })),
    )
        .into_response()
}

async fn handle(State(s): State<Arc<Sentinel>>, headers: HeaderMap, body: Bytes) -> Response {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim);
    let Some((token_id, account, scope)) = token.and_then(|t| s.auth.api_token(t)) else {
        return unauthorized("Send a Sentinel API token as `Authorization: Bearer snt_...`. Create one on the Alerts page.");
    };
    let ctx = Ctx { account, token_id, write: scope == "write" };
    if !s.limits.allow_mcp(token_id, false) {
        return (StatusCode::TOO_MANY_REQUESTS, Json(json!({ "error": "Too many requests; slow down" }))).into_response();
    }

    let request: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return Json(rpc_error(Value::Null, -32700, "Parse error")).into_response(),
    };
    match request {
        Value::Array(batch) => {
            let mut out = Vec::new();
            for item in batch.into_iter().take(20) {
                if let Some(r) = dispatch(&s, &ctx, item).await {
                    out.push(r);
                }
            }
            if out.is_empty() {
                StatusCode::ACCEPTED.into_response()
            } else {
                Json(Value::Array(out)).into_response()
            }
        }
        single => match dispatch(&s, &ctx, single).await {
            Some(r) => Json(r).into_response(),
            None => StatusCode::ACCEPTED.into_response(),
        },
    }
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Answers one JSON-RPC message. Notifications (no id) get no answer.
async fn dispatch(s: &Arc<Sentinel>, ctx: &Ctx, req: Value) -> Option<Value> {
    let id = req.get("id").cloned();
    let method = req.get("method").and_then(Value::as_str).unwrap_or_default().to_string();
    let params = req.get("params").cloned().unwrap_or(Value::Null);
    let Some(id) = id else { return None };

    let result: Result<Value, (i64, String)> = match method.as_str() {
        "initialize" => {
            let wanted = params["protocolVersion"].as_str().unwrap_or_default();
            let version = SUPPORTED.iter().find(|v| **v == wanted).unwrap_or(&SUPPORTED[0]);
            Ok(json!({
                "protocolVersion": version,
                "capabilities": { "tools": {}, "resources": {}, "prompts": {} },
                "serverInfo": { "name": "vortex-sentinel", "version": env!("CARGO_PKG_VERSION") },
                "instructions": INSTRUCTIONS,
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_list(ctx.write) })),
        "tools/call" => Ok(call_tool(s, ctx, &params).await),
        "resources/list" => Ok(resource_list(s)),
        "resources/templates/list" => Ok(resource_templates()),
        "resources/read" => read_resource(s, &params).await.map_err(|e| (-32002, e)),
        "prompts/list" => Ok(json!({ "prompts": prompts() })),
        "prompts/get" => get_prompt(&params).map_err(|e| (-32602, e)),
        _ => Err((-32601, format!("Method not found: {method}"))),
    };
    Some(match result {
        Ok(r) => rpc_result(id, r),
        Err((code, message)) => rpc_error(id, code, &message),
    })
}

// ------------------------------------------------------------------- tools

struct Tool {
    name: &'static str,
    description: &'static str,
    schema: Value,
    write: bool,
}

fn object(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required, "additionalProperties": false })
}

fn tools() -> Vec<Tool> {
    let program = json!({ "type": "string", "description": "Program address or its label, e.g. \"Pump.fun\"." });
    let incident_id = json!({ "type": "integer", "description": "Incident number, e.g. 1001." });
    vec![
        Tool {
            name: "list_programs",
            description: "Every monitored program with its live health, throughput, failure rate and open incidents. Start here.",
            schema: object(json!({}), &[]),
            write: false,
        },
        Tool {
            name: "get_health",
            description: "A 0-100 health score for a program with each check (failure rate, open incidents, liveness, compute, funds, decoding, upgrade authority) stating the rule and numbers it used.",
            schema: object(json!({ "program": program }), &["program"]),
            write: false,
        },
        Tool {
            name: "get_summary",
            description: "What a program did over a period: transactions, success rate, unique wallets, value moved, top instructions and errors, incidents with time to detect and resolve, program changes, and suggested actions.",
            schema: object(
                json!({ "program": program, "period": { "type": "string", "description": "1h to 30d, like 24h or 7d. Default 24h." } }),
                &["program"],
            ),
            write: false,
        },
        Tool {
            name: "list_incidents",
            description: "Recent incidents, newest first. Filter by program, status or kind.",
            schema: object(
                json!({
                    "program": program,
                    "status": { "type": "string", "enum": ["open", "resolved", "all"], "description": "Default open." },
                    "kind": { "type": "string", "description": "failure_spike, error_spike, activity_drop, activity_spike, compute_spike, large_transfer, rule_triggered, authority_change or vault_drain." },
                    "limit": { "type": "integer", "description": "Up to 50. Default 20." }
                }),
                &[],
            ),
            write: false,
        },
        Tool {
            name: "get_incident",
            description: "One incident in full: the rule that fired with its numbers, evidence (failure fingerprints, correlated upgrade, vault or authority details) and example transactions.",
            schema: object(json!({ "id": incident_id }), &["id"]),
            write: false,
        },
        Tool {
            name: "diagnose_incident",
            description: "A structured diagnosis of an incident: likely cause, confidence, the evidence behind it, similar earlier incidents and concrete next steps. Deterministic, built only from recorded data.",
            schema: object(json!({ "id": incident_id }), &["id"]),
            write: false,
        },
        Tool {
            name: "get_incident_report",
            description: "A markdown post-mortem for an incident: timeline, impact, failures, diagnosis and next steps.",
            schema: object(json!({ "id": incident_id }), &["id"]),
            write: false,
        },
        Tool {
            name: "explain_transaction",
            description: "What a transaction did, in plain language: narrative, value flow between labelled parties, call tree, decoded instructions and why it failed. Works for any mainnet signature.",
            schema: object(json!({ "signature": { "type": "string" } }), &["signature"]),
            write: false,
        },
        Tool {
            name: "get_posture",
            description: "Who can upgrade a program: upgrade authority, whether it is a single key or program-controlled (multisig), last deploy, and the risks that follow.",
            schema: object(json!({ "program": program }), &["program"]),
            write: false,
        },
        Tool {
            name: "list_vaults",
            description: "The vault accounts watched for drains with balances and recent net flow, plus accounts that look like vaults but are not watched yet.",
            schema: object(json!({ "program": program }), &["program"]),
            write: false,
        },
        Tool {
            name: "list_dependencies",
            description: "The programs a program calls (oracles, AMMs, ...), how often, and which are watched for upgrades.",
            schema: object(json!({ "program": program }), &["program"]),
            write: false,
        },
        Tool {
            name: "list_rules",
            description: "The token owner's alert rules (secrets are masked).",
            schema: object(json!({}), &[]),
            write: false,
        },
        Tool {
            name: "list_deliveries",
            description: "Recent alert and summary deliveries for the token owner: which channel, event, and whether it arrived.",
            schema: object(json!({ "limit": { "type": "integer", "description": "Up to 100. Default 20." } }), &[]),
            write: false,
        },
        Tool {
            name: "watch_program",
            description: "Start monitoring a program for the token owner.",
            schema: object(
                json!({ "program_id": { "type": "string", "description": "Program address." }, "label": { "type": "string" } }),
                &["program_id"],
            ),
            write: true,
        },
        Tool {
            name: "create_rule",
            description: "Create an alert rule. `condition` is one of {type:\"metric\",metric,op,value,window_secs}, {type:\"transfer_usd\",min_usd}, {type:\"transfer\",mint,min_amount} or {type:\"incident\",kinds:[],min_severity}. \
Give `channels` (slack/telegram/pagerduty/discord/webhook) or `channels_from_rule` to reuse an existing rule's channels without handling their secrets.",
            schema: object(
                json!({
                    "name": { "type": "string" },
                    "program": { "type": "string", "description": "Program address or label; omit for every program you watch." },
                    "condition": { "type": "object" },
                    "severity": { "type": "string", "enum": ["info", "low", "medium", "high", "critical"] },
                    "channels": { "type": "array", "items": { "type": "object" } },
                    "channels_from_rule": { "type": "integer", "description": "Copy channels (with their secrets) from one of your rules." },
                    "create_incident": { "type": "boolean" },
                    "cooldown_secs": { "type": "integer" }
                }),
                &["name", "condition"],
            ),
            write: true,
        },
        Tool {
            name: "suggest_rules",
            description: "Alert rules worth having for a program, read from its Anchor IDL: authority and admin changes, pause controls, funds leaving (with a size threshold you must choose), configuration changes, and the events that announce them. Each says why. Nothing is created; show them to the user, then create the ones they pick with apply_suggestions.",
            schema: object(json!({ "program": program }), &["program"]),
            write: false,
        },
        Tool {
            name: "apply_suggestions",
            description: "Create suggested rules by id (from suggest_rules) on the channels given, or copied from one of your rules with channels_from_rule. A suggestion that needs a number (a size threshold) must have one in `values`, keyed by suggestion id, in the token's smallest unit. Rules that already exist are left alone.",
            schema: object(
                json!({
                    "program": program,
                    "ids": { "type": "array", "items": { "type": "string" } },
                    "values": { "type": "object", "description": "Thresholds by suggestion id, e.g. {\"instruction:large:withdraw\": 5000000000}" },
                    "channels": { "type": "array", "items": { "type": "object" } },
                    "channels_from_rule": { "type": "integer" }
                }),
                &["program", "ids"],
            ),
            write: true,
        },
        Tool {
            name: "protect_program",
            description: "Set up the alerts most teams want for a program on the channels given (or copied from one of your rules with channels_from_rule): any incident of high severity or above, failure rate above 20%, an admin instruction from a new wallet, health below 60, and Sentinel's own feed problems. Rules that already exist are left alone. Returns what was created and what to do next.",
            schema: object(
                json!({
                    "program": program,
                    "channels": { "type": "array", "items": { "type": "object" } },
                    "channels_from_rule": { "type": "integer", "description": "Reuse the channels of one of your rules." }
                }),
                &["program"],
            ),
            write: true,
        },
        Tool {
            name: "test_rule",
            description: "Send a test delivery through a rule's channels.",
            schema: object(json!({ "id": { "type": "integer" } }), &["id"]),
            write: true,
        },
        Tool {
            name: "update_incident_status",
            description: "Mark an incident investigating or resolved. Requires watching the program.",
            schema: object(
                json!({ "id": incident_id, "status": { "type": "string", "enum": ["investigating", "resolved"] } }),
                &["id", "status"],
            ),
            write: true,
        },
        Tool {
            name: "set_vaults",
            description: "Replace the vault accounts watched for drains on a program (up to 10). Shared with everyone watching the program.",
            schema: object(
                json!({ "program": program, "vaults": { "type": "array", "items": { "type": "string" } } }),
                &["program", "vaults"],
            ),
            write: true,
        },
        Tool {
            name: "mute_program",
            description: "Hold notifications for a program for a while (a maintenance window, e.g. during a deploy). Incidents are still recorded and a resolution is still sent. 0 minutes lifts it.",
            schema: object(
                json!({ "program": program, "minutes": { "type": "integer", "description": "0 to 10080 (a week)." }, "reason": { "type": "string" } }),
                &["program", "minutes"],
            ),
            write: true,
        },
        Tool {
            name: "send_summary_now",
            description: "Deliver a scheduled summary to its channels immediately.",
            schema: object(json!({ "schedule_id": { "type": "integer" } }), &["schedule_id"]),
            write: true,
        },
    ]
}

fn tool_list(write_scope: bool) -> Vec<Value> {
    tools()
        .into_iter()
        .map(|t| {
            let note = if t.write && !write_scope { " (needs a write-scoped token)" } else { "" };
            json!({
                "name": t.name,
                "description": format!("{}{note}", t.description),
                "inputSchema": t.schema,
                "annotations": { "readOnlyHint": !t.write, "openWorldHint": false },
            })
        })
        .collect()
}

fn ok_result(value: Value) -> Value {
    // structuredContent must be an object.
    let structured = if value.is_object() { value.clone() } else { json!({ "result": value.clone() }) };
    json!({
        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&value).unwrap_or_default() }],
        "structuredContent": structured,
    })
}

fn text_result(text: String) -> Value {
    json!({ "content": [{ "type": "text", "text": text }] })
}

fn tool_error(message: impl Into<String>) -> Value {
    json!({ "content": [{ "type": "text", "text": message.into() }], "isError": true })
}

fn api_err(e: ApiError) -> String {
    crate::redact::scrub(&e.1)
}

fn any_err(e: impl std::fmt::Display) -> String {
    crate::redact::scrub(&e.to_string())
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args[key]
        .as_str()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("`{key}` is required"))
}

fn id_arg(args: &Value, key: &str) -> Result<i64, String> {
    args[key].as_i64().ok_or_else(|| format!("`{key}` must be an integer"))
}

/// A monitored program from its address or its label.
fn resolve_program(s: &Sentinel, text: &str) -> Result<String, String> {
    let programs = s.programs();
    if let Some(p) = programs.iter().find(|p| p.program_id == text) {
        return Ok(p.program_id.clone());
    }
    let wanted = text.to_lowercase();
    let by_label: Vec<_> = programs.iter().filter(|p| p.label.to_lowercase() == wanted).collect();
    match by_label.as_slice() {
        [one] => Ok(one.program_id.clone()),
        [] => Err(format!(
            "No monitored program matches \"{text}\". Monitored: {}. Use watch_program to add one.",
            programs.iter().map(|p| format!("{} ({})", p.label, p.program_id)).collect::<Vec<_>>().join(", ")
        )),
        many => Err(format!("\"{text}\" matches {} programs; use the address", many.len())),
    }
}

async fn call_tool(s: &Arc<Sentinel>, ctx: &Ctx, params: &Value) -> Value {
    let name = params["name"].as_str().unwrap_or_default();
    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
    let Some(tool) = tools().into_iter().find(|t| t.name == name) else {
        return tool_error(format!("Unknown tool \"{name}\""));
    };
    if tool.write {
        if !ctx.write {
            return tool_error("This token is read-only. Create a write-scoped token on the Alerts page to change things.");
        }
        if !s.limits.allow_mcp(ctx.token_id, true) {
            return tool_error("Too many changes from this token in the last minute; wait and try again.");
        }
    }
    match run_tool(s, ctx, name, &args).await {
        Ok(Output::Json(v)) => ok_result(v),
        Ok(Output::Text(t)) => text_result(t),
        Err(message) => tool_error(message),
    }
}

enum Output {
    Json(Value),
    Text(String),
}

fn json_out(v: Value) -> Result<Output, String> {
    Ok(Output::Json(v))
}

async fn run_tool(s: &Arc<Sentinel>, ctx: &Ctx, name: &str, args: &Value) -> Result<Output, String> {
    let account = ctx.account.as_str();
    match name {
        "list_programs" => {
            let mine = s.watching(account);
            let rows: Vec<Value> = s
                .programs()
                .into_iter()
                .map(|p| {
                    json!({
                        "program_id": p.program_id,
                        "label": p.label,
                        "health": p.health,
                        "tps_10s": p.tps_10s,
                        "transactions_60s": p.tx_60s,
                        "failure_rate_60s": p.failure_rate_60s,
                        "baseline_failure_rate": p.baseline_failure_rate,
                        "open_incidents": p.open_incidents,
                        "last_transaction_at": p.last_tx_at,
                        "watched_by_you": mine.contains(&p.program_id),
                    })
                })
                .collect();
            json_out(json!({ "programs": rows }))
        }
        "get_health" => {
            let id = resolve_program(s, str_arg(args, "program")?)?;
            let _ = s.posture(&id).await;
            json_out(json!(s.health(&id).ok_or("program not monitored")?))
        }
        "get_summary" => {
            let id = resolve_program(s, str_arg(args, "program")?)?;
            let period = api::parse_period(args["period"].as_str()).ok_or("period must be 1h to 30d, like 24h or 7d")?;
            let _ = s.posture(&id).await;
            let summary = s.summary(&id, period).map_err(any_err)?;
            let facts: Vec<Value> = summary.facts().into_iter().map(|(k, v)| json!({ k: v })).collect();
            let mut v = json!(summary);
            v["facts"] = json!(facts);
            json_out(v)
        }
        "list_incidents" => {
            let program = match args["program"].as_str().filter(|p| !p.trim().is_empty()) {
                Some(p) => Some(resolve_program(s, p.trim())?),
                None => None,
            };
            let status = args["status"].as_str().unwrap_or("open");
            let kind = args["kind"].as_str().map(str::to_string);
            let limit = args["limit"].as_i64().unwrap_or(20).clamp(1, 50) as usize;
            let labels = s.program_labels();
            let rows: Vec<Value> = s
                .store
                .incidents(program.as_deref(), 300)
                .map_err(any_err)?
                .into_iter()
                .filter(|i| match status {
                    "open" => i.status != IncidentStatus::Resolved,
                    "resolved" => i.status == IncidentStatus::Resolved,
                    _ => true,
                })
                .filter(|i| kind.as_deref().is_none_or(|k| i.kind.as_str() == k))
                .take(limit)
                .map(|i| {
                    json!({
                        "id": i.id,
                        "program_id": i.program_id,
                        "program": labels.get(&i.program_id),
                        "kind": i.kind.as_str(),
                        "severity": i.severity,
                        "status": i.status,
                        "title": i.title,
                        "summary": i.summary,
                        "detected_at": i.detected_at,
                        "resolved_at": i.resolved_at,
                        "affected_transactions": i.affected_count,
                        "affected_wallets": i.affected_wallets,
                        "after_upgrade": i.evidence.get("deploy").is_some(),
                    })
                })
                .collect();
            json_out(json!({ "incidents": rows }))
        }
        "get_incident" => {
            let id = id_arg(args, "id")?;
            let incident = s.store.incident(id).map_err(any_err)?.ok_or("incident not found")?;
            let txs: Vec<Value> = s
                .store
                .incident_transactions(id, 10)
                .map_err(any_err)?
                .into_iter()
                .map(|t| json!({ "signature": t.signature, "success": t.success, "error": t.error, "instructions": t.instructions, "fee_payer": t.fee_payer }))
                .collect();
            json_out(json!({
                "incident": incident,
                "program": s.program_labels().get(&incident.program_id),
                "example_transactions": txs,
                "report_url": format!("{}/api/incidents/{id}/report", s.public_url.trim_end_matches('/')),
            }))
        }
        "diagnose_incident" => {
            let id = id_arg(args, "id")?;
            json_out(json!(s.diagnose_incident(id).map_err(any_err)?.ok_or("incident not found")?))
        }
        "get_incident_report" => {
            let id = id_arg(args, "id")?;
            Ok(Output::Text(s.incident_report(id).map_err(any_err)?.ok_or("incident not found")?))
        }
        "explain_transaction" => {
            let sig = str_arg(args, "signature")?;
            let found = s.explain_transaction(sig).await.map_err(any_err)?.ok_or(
                "Transaction not found in Sentinel's window or incidents, and RPC does not have it (or SOLANA_RPC_URL is not set).",
            )?;
            // The trace is the explanation; the raw transaction is too large to be useful here.
            let tx = &found["transaction"];
            json_out(json!({
                "signature": sig,
                "success": tx["success"],
                "error": tx["error"],
                "fee": tx["fee"],
                "narrative": found["trace"]["narrative"],
                "value_flow": found["trace"]["flows"],
                "balance_changes": found["trace"]["balance_changes"],
                "parties": found["trace"]["parties"],
                "call_tree": found["trace"]["call_tree"],
                "decoded_instructions": found["trace"]["decoded"],
                "error_detail": found["trace"]["error_detail"],
                "monitored_programs": found["monitored_programs"],
                "incidents": found["incidents"],
            }))
        }
        "get_posture" => {
            let id = resolve_program(s, str_arg(args, "program")?)?;
            json_out(json!(s.posture(&id).await.map_err(any_err)?))
        }
        "list_vaults" => {
            let id = resolve_program(s, str_arg(args, "program")?)?;
            json_out(s.vault_status(&id).ok_or("program not monitored")?)
        }
        "list_dependencies" => {
            let id = resolve_program(s, str_arg(args, "program")?)?;
            json_out(s.dependencies(&id).ok_or("program not monitored")?)
        }
        "list_rules" => {
            let rules: Vec<_> = s
                .store
                .rules()
                .map_err(any_err)?
                .into_iter()
                .filter(|r| r.owner.as_deref() == Some(account))
                .map(|r| r.masked())
                .collect();
            json_out(json!({ "rules": rules }))
        }
        "list_deliveries" => {
            let limit = args["limit"].as_i64().unwrap_or(20).clamp(1, 100);
            json_out(json!({ "deliveries": s.store.executions_for(account, limit).map_err(any_err)? }))
        }
        "watch_program" => {
            let id = str_arg(args, "program_id")?.to_string();
            let label = args["label"].as_str().map(str::to_string);
            json_out(api::add_program_inner(s, account, id, label).map_err(api_err)?)
        }
        "create_rule" => {
            let mut input = Map::new();
            input.insert("name".into(), args["name"].clone());
            input.insert("condition".into(), args["condition"].clone());
            if let Some(p) = args["program"].as_str().filter(|p| !p.trim().is_empty()) {
                input.insert("program_id".into(), json!(resolve_program(s, p.trim())?));
            }
            for key in ["severity", "create_incident", "cooldown_secs"] {
                if !args[key].is_null() {
                    input.insert(key.into(), args[key].clone());
                }
            }
            if let Some(from) = args["channels_from_rule"].as_i64() {
                let source = s
                    .store
                    .rules()
                    .map_err(any_err)?
                    .into_iter()
                    .find(|r| r.id == from && r.owner.as_deref() == Some(account))
                    .ok_or("channels_from_rule: no such rule of yours")?;
                input.insert("channels".into(), json!(source.targets()));
            } else if !args["channels"].is_null() {
                input.insert("channels".into(), args["channels"].clone());
            }
            let input: RuleInput = serde_json::from_value(Value::Object(input)).map_err(|e| format!("Invalid rule: {e}"))?;
            json_out(api::create_rule_inner(s, account.to_string(), input).await.map_err(api_err)?)
        }
        "suggest_rules" => {
            let id = resolve_program(s, str_arg(args, "program")?)?;
            json_out(api::suggestions_inner(s, &id).await.map_err(api_err)?)
        }
        "apply_suggestions" => {
            let id = resolve_program(s, str_arg(args, "program")?)?;
            let ids: Vec<String> = serde_json::from_value(args["ids"].clone()).map_err(|_| "`ids` must be a list of suggestion ids".to_string())?;
            let values: std::collections::HashMap<String, serde_json::Number> = match args.get("values").filter(|v| !v.is_null()) {
                Some(v) => serde_json::from_value(v.clone()).map_err(|_| "`values` must map suggestion ids to numbers".to_string())?,
                None => Default::default(),
            };
            let channels: Vec<crate::model::Channel> = match args.get("channels").filter(|c| !c.is_null()) {
                Some(c) => serde_json::from_value(c.clone()).map_err(|e| format!("Invalid channels: {e}"))?,
                None => Vec::new(),
            };
            json_out(api::apply_suggestions_inner(s, account, &id, ids, values, channels, args["channels_from_rule"].as_i64()).await.map_err(api_err)?)
        }
        "protect_program" => {
            let id = resolve_program(s, str_arg(args, "program")?)?;
            let channels: Vec<crate::model::Channel> = match args.get("channels").filter(|c| !c.is_null()) {
                Some(c) => serde_json::from_value(c.clone()).map_err(|e| format!("Invalid channels: {e}"))?,
                None => Vec::new(),
            };
            json_out(api::protect_inner(s, account, &id, channels, args["channels_from_rule"].as_i64()).await.map_err(api_err)?)
        }
        "test_rule" => {
            let id = id_arg(args, "id")?;
            let rule = s
                .store
                .rules()
                .map_err(any_err)?
                .into_iter()
                .find(|r| r.id == id && r.owner.as_deref() == Some(account))
                .ok_or("rule not found")?;
            if !rule.has_targets() {
                return Err("That rule has no channel to deliver to".into());
            }
            s.test_rule(rule);
            json_out(json!({ "queued": true }))
        }
        "update_incident_status" => {
            let id = id_arg(args, "id")?;
            let status = match args["status"].as_str() {
                Some("investigating") => IncidentStatus::Investigating,
                Some("resolved") => IncidentStatus::Resolved,
                _ => return Err("status must be investigating or resolved".into()),
            };
            let incident = s.store.incident(id).map_err(any_err)?.ok_or("incident not found")?;
            if !s.limits.is_admin(account) && !s.is_watching(account, &incident.program_id) {
                return Err("Watch this program to update its incidents".into());
            }
            json_out(json!(s.set_incident_status(id, status).map_err(any_err)?))
        }
        "set_vaults" => {
            let id = resolve_program(s, str_arg(args, "program")?)?;
            let vaults: Vec<String> = args["vaults"]
                .as_array()
                .ok_or("`vaults` must be a list of account addresses")?
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
            json_out(api::set_vaults_inner(s, account, &id, &vaults).map_err(api_err)?)
        }
        "mute_program" => {
            let id = resolve_program(s, str_arg(args, "program")?)?;
            let minutes = args["minutes"].as_u64().ok_or("`minutes` must be a whole number")?.min(7 * 24 * 60) as u32;
            json_out(api::mute_inner(s, account, &id, minutes, args["reason"].as_str().map(String::from)).map_err(api_err)?)
        }
        "send_summary_now" => {
            let id = id_arg(args, "schedule_id")?;
            let schedule = s
                .store
                .schedules()
                .map_err(any_err)?
                .into_iter()
                .find(|x| x.id == id && x.owner == account)
                .ok_or("schedule not found")?;
            s.send_summary(&schedule).map_err(any_err)?;
            json_out(json!({ "queued": true }))
        }
        other => Err(format!("Unknown tool \"{other}\"")),
    }
}

// --------------------------------------------------------------- resources

fn resource_templates() -> Value {
    json!({ "resourceTemplates": [
        { "uriTemplate": "sentinel://program/{program}/health", "name": "Program health", "description": "The health check for a program.", "mimeType": "application/json" },
        { "uriTemplate": "sentinel://program/{program}/summary", "name": "Program summary (24h)", "description": "What a program did in the last 24 hours.", "mimeType": "application/json" },
        { "uriTemplate": "sentinel://incident/{id}/report", "name": "Incident post-mortem", "description": "Markdown post-mortem for an incident.", "mimeType": "text/markdown" },
        { "uriTemplate": "sentinel://incident/{id}/diagnosis", "name": "Incident diagnosis", "description": "Structured diagnosis of an incident.", "mimeType": "application/json" },
    ] })
}

fn resource_list(s: &Sentinel) -> Value {
    let mut out = Vec::new();
    for p in s.programs().into_iter().take(25) {
        out.push(json!({ "uri": format!("sentinel://program/{}/health", p.program_id), "name": format!("{} health", p.label), "mimeType": "application/json" }));
        out.push(json!({ "uri": format!("sentinel://program/{}/summary", p.program_id), "name": format!("{} summary (24h)", p.label), "mimeType": "application/json" }));
    }
    if let Ok(incidents) = s.store.incidents(None, 100) {
        for i in incidents.into_iter().filter(|i| i.status != IncidentStatus::Resolved).take(20) {
            out.push(json!({ "uri": format!("sentinel://incident/{}/report", i.id), "name": format!("Incident #{} post-mortem", i.id), "description": i.summary, "mimeType": "text/markdown" }));
        }
    }
    json!({ "resources": out })
}

async fn read_resource(s: &Arc<Sentinel>, params: &Value) -> Result<Value, String> {
    let uri = params["uri"].as_str().ok_or("uri is required")?;
    let path = uri.strip_prefix("sentinel://").ok_or("unknown resource")?;
    let parts: Vec<&str> = path.split('/').collect();
    let (mime, text) = match parts.as_slice() {
        ["program", program, "health"] => {
            let id = resolve_program(s, program)?;
            let _ = s.posture(&id).await;
            ("application/json", serde_json::to_string_pretty(&s.health(&id).ok_or("program not monitored")?).unwrap_or_default())
        }
        ["program", program, "summary"] => {
            let id = resolve_program(s, program)?;
            ("application/json", serde_json::to_string_pretty(&s.summary(&id, 24 * 3600).map_err(any_err)?).unwrap_or_default())
        }
        ["incident", id, "report"] => {
            let id: i64 = id.parse().map_err(|_| "incident id must be a number")?;
            ("text/markdown", s.incident_report(id).map_err(any_err)?.ok_or("incident not found")?)
        }
        ["incident", id, "diagnosis"] => {
            let id: i64 = id.parse().map_err(|_| "incident id must be a number")?;
            ("application/json", serde_json::to_string_pretty(&s.diagnose_incident(id).map_err(any_err)?.ok_or("incident not found")?).unwrap_or_default())
        }
        _ => return Err(format!("Unknown resource {uri}")),
    };
    Ok(json!({ "contents": [{ "uri": uri, "mimeType": mime, "text": text }] }))
}

// ----------------------------------------------------------------- prompts

fn prompts() -> Vec<Value> {
    vec![
        json!({
            "name": "triage-incident",
            "description": "Work out what happened in a Sentinel incident and what to do about it.",
            "arguments": [{ "name": "incident_id", "description": "Incident number", "required": true }],
        }),
        json!({
            "name": "daily-standup",
            "description": "A short status update on one or all monitored programs, ready to post to a team channel.",
            "arguments": [{ "name": "program", "description": "Program address or label; omit for all", "required": false }],
        }),
        json!({
            "name": "setup-protection",
            "description": "Set up monitoring and alerts for a program, step by step.",
            "arguments": [{ "name": "program_id", "description": "Program address", "required": true }],
        }),
    ]
}

fn get_prompt(params: &Value) -> Result<Value, String> {
    let args = &params["arguments"];
    let text = match params["name"].as_str().unwrap_or_default() {
        "triage-incident" => {
            let id = args["incident_id"].as_str().map(str::to_string).or_else(|| args["incident_id"].as_i64().map(|n| n.to_string())).ok_or("incident_id is required")?;
            format!(
                "Triage Sentinel incident #{id}.\n\n\
                 1. Call diagnose_incident and get_incident for #{id}.\n\
                 2. Call explain_transaction on two or three of the example transactions, including a failed one.\n\
                 3. If the diagnosis mentions an upgrade, call get_posture for the program.\n\n\
                 Then write: what happened; the likely cause with the confidence the diagnosis gave; who is affected (wallets, transactions); what to do right now; and what to do afterwards. \
                 Quote the numbers from the tools. If you go beyond the recorded data, say so."
            )
        }
        "daily-standup" => match args["program"].as_str().filter(|p| !p.is_empty()) {
            Some(p) => format!("Write a standup update for {p}. Call get_summary (24h) and get_health for it, and list_incidents with status \"all\". Cover activity, reliability, anything that changed on chain, and what needs attention. Keep it under 150 words."),
            None => "Write a standup update for every monitored program. Call list_programs, then get_summary (24h) and get_health for each. Lead with anything unhealthy, then one line per program. Keep it under 200 words.".to_string(),
        },
        "setup-protection" => {
            let p = args["program_id"].as_str().ok_or("program_id is required")?;
            format!(
                "Set up protection for program {p} using Sentinel's tools.\n\n\
                 1. watch_program (if it is not monitored), then get_posture and report who can upgrade it.\n\
                 2. list_vaults: if none are watched and candidates exist, ask which are the program's treasury or vaults, then set_vaults.\n\
                 3. list_rules to see which channels already exist. Reuse one with channels_from_rule rather than asking for new secrets.\n\
                 4. create_rule for: any incident of severity high or above; a failure rate above 20% over 60s.\n\
                 5. test_rule on each new rule and confirm delivery with list_deliveries.\n\n\
                 Ask before creating anything, and never ask the user to paste a bot token or routing key into chat."
            )
        }
        other => return Err(format!("Unknown prompt \"{other}\"")),
    };
    Ok(json!({ "messages": [{ "role": "user", "content": { "type": "text", "text": text } }] }))
}

