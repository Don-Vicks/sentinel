//! Lets MCP clients that start a local command (Claude Desktop, Cursor, ...) talk
//! to a Sentinel server. Reads JSON-RPC messages from stdin, one per line, sends
//! each to `$SENTINEL_URL/mcp` with `$SENTINEL_TOKEN`, and prints the replies.
//!
//!   SENTINEL_URL=https://your-sentinel SENTINEL_TOKEN=snt_... sentinel-mcp

use serde_json::{json, Value};
use std::io::Write;
use tokio::io::{AsyncBufReadExt, BufReader};

fn fail(id: Value, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": message } })
}

#[tokio::main]
async fn main() {
    let url = match std::env::var("SENTINEL_URL") {
        Ok(u) if !u.trim().is_empty() => format!("{}/mcp", u.trim().trim_end_matches('/')),
        _ => {
            eprintln!("Set SENTINEL_URL to your Sentinel server, e.g. https://sentinel.example");
            std::process::exit(2);
        }
    };
    let token = match std::env::var("SENTINEL_TOKEN") {
        Ok(t) if t.starts_with("snt_") => t,
        _ => {
            eprintln!("Set SENTINEL_TOKEN to an API token (snt_...) created on the Alerts page");
            std::process::exit(2);
        }
    };
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .user_agent("sentinel-mcp")
        .build()
        .expect("http client");

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let stdout = std::io::stdout();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => {
                emit(&stdout, &json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": "Parse error" } }));
                continue;
            }
        };
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let reply = client.post(&url).bearer_auth(&token).json(&request).send().await;
        match reply {
            Ok(res) if res.status() == reqwest::StatusCode::ACCEPTED => {}
            Ok(res) if res.status() == reqwest::StatusCode::UNAUTHORIZED => {
                emit(&stdout, &fail(id, "Sentinel rejected the token. Create a new one on the Alerts page."));
            }
            Ok(res) => match res.json::<Value>().await {
                Ok(body) => emit(&stdout, &body),
                Err(_) => emit(&stdout, &fail(id, "Sentinel sent a reply that was not JSON")),
            },
            // Only say the host: the URL is configuration, but keep error text short.
            Err(e) => emit(&stdout, &fail(id, &format!("Could not reach Sentinel: {}", e.without_url()))),
        }
    }
}

fn emit(out: &std::io::Stdout, value: &Value) {
    let mut handle = out.lock();
    let _ = writeln!(handle, "{value}");
    let _ = handle.flush();
}
