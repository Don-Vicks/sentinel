//! The authority lookup against a mock RPC that behaves like Solami's:
//! `getProgramAccountsV2` with partial (even empty) pages and a pagination key,
//! plus an endpoint with no V2 at all, like the public RPC.

use axum::{routing::post, Json, Router};
use sentinel::resolve::programs_by_authority;
use serde_json::{json, Value};
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

struct World {
    authority: Pubkey,
    /// ProgramData address -> (program, empty pages before it shows up)
    owners: HashMap<String, (Pubkey, usize)>,
    v2: bool,
}

fn account(k: &str) -> Value {
    json!({ "pubkey": k, "account": { "data": ["", "base64"], "executable": false, "lamports": 1, "owner": "x" } })
}

async fn serve(world: Arc<World>) -> String {
    let app = Router::new().route(
        "/",
        post(move |Json(req): Json<Value>| {
            let w = world.clone();
            async move {
                let method = req["method"].as_str().unwrap().to_string();
                let cfg = &req["params"][1];
                if method == "getProgramAccountsV2" && !w.v2 {
                    return Json(json!({ "jsonrpc": "2.0", "id": 1, "error": { "code": -32601, "message": "Method not found" } }));
                }
                let filters = cfg["filters"].as_array().unwrap();
                // Authority listing carries a memcmp at offset 13; the owner lookup's is at offset 4.
                let memcmp = filters
                    .iter()
                    .filter_map(|f| f.get("memcmp"))
                    .find(|m| m["offset"] == 13 || m["offset"] == 4)
                    .unwrap();
                let wanted = memcmp["bytes"].as_str().unwrap();
                let all: Vec<Value>;
                let mut next = Value::Null;
                if memcmp["offset"] == 13 {
                    // Listing by authority.
                    assert_eq!(wanted, w.authority.to_string());
                    all = w.owners.keys().map(|k| account(k)).collect();
                } else {
                    // Which program owns this ProgramData? Empty pages first, like a scan window.
                    let (program, empties) = w.owners.get(wanted).cloned().expect("known program data");
                    let page: usize = cfg["paginationKey"].as_str().map(|k| k.parse().unwrap()).unwrap_or(0);
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    if method == "getProgramAccountsV2" && page < empties {
                        next = json!((page + 1).to_string());
                        all = vec![];
                    } else {
                        all = vec![account(&program.to_string())];
                    }
                }
                let result = if method == "getProgramAccountsV2" {
                    json!({ "context": { "slot": 1 }, "value": { "accounts": all, "paginationKey": next } })
                } else {
                    json!(all)
                };
                Json(json!({ "jsonrpc": "2.0", "id": 1, "result": result }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}/")
}

fn world(v2: bool, slow_empties: usize) -> (Arc<World>, Vec<Pubkey>) {
    let authority = Pubkey::new_unique();
    let programs: Vec<Pubkey> = (0..3).map(|_| Pubkey::new_unique()).collect();
    let mut owners = HashMap::new();
    for (i, p) in programs.iter().enumerate() {
        // The first program's owner lookup walks several empty pages.
        owners.insert(Pubkey::new_unique().to_string(), (*p, if i == 0 { slow_empties } else { 0 }));
    }
    (Arc::new(World { authority, owners, v2 }), programs)
}

fn ids(owned: &sentinel::resolve::Owned) -> Vec<String> {
    let mut v: Vec<String> = owned.programs.iter().map(|c| c.program_id.clone()).collect();
    v.sort();
    v
}

#[tokio::test]
async fn follows_empty_pages_and_reports_progress() {
    let (w, programs) = world(true, 4); // ~750 ms of empty pages for the first program
    let url = serve(w.clone()).await;

    // A short budget returns what's ready and says how much is still pending.
    let first = programs_by_authority(&url, &w.authority, Duration::from_millis(400)).await.unwrap();
    assert_eq!(first.pending, 1, "the slow lookup is still running in the background");
    assert_eq!(first.programs.len(), 2);

    // Asking again picks up the background result; nothing is looked up twice.
    let second = programs_by_authority(&url, &w.authority, Duration::from_secs(5)).await.unwrap();
    assert_eq!((second.pending, second.missing), (0, 0));
    let mut want: Vec<String> = programs.iter().map(|p| p.to_string()).collect();
    want.sort();
    assert_eq!(ids(&second), want);
}

#[tokio::test]
async fn works_without_v2_like_the_public_rpc() {
    let (w, programs) = world(false, 0);
    let url = serve(w.clone()).await;
    let owned = programs_by_authority(&url, &w.authority, Duration::from_secs(5)).await.unwrap();
    assert_eq!((owned.pending, owned.missing), (0, 0));
    assert_eq!(owned.programs.len(), programs.len());
}

#[tokio::test]
async fn unknown_authority_has_no_programs_and_a_dead_rpc_is_an_error() {
    let (w, _) = world(true, 0);
    let url = serve(w).await;
    let nobody = Pubkey::new_unique();
    // The mock asserts the authority matches, so point at an empty world instead.
    let empty = Arc::new(World { authority: nobody, owners: HashMap::new(), v2: true });
    let owned = programs_by_authority(&serve(empty).await, &nobody, Duration::from_secs(1)).await.unwrap();
    assert!(owned.programs.is_empty() && owned.pending == 0);
    let _ = url;

    assert!(programs_by_authority("http://127.0.0.1:1/", &nobody, Duration::from_secs(1)).await.is_err());
}
