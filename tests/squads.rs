//! An upgrade executed by a Squads multisig says so, and says how many signatures it takes.

use chrono::{TimeZone, Utc};
use sentinel::engine::Sentinel;
use sentinel::model::*;
use sentinel::posture::programdata_address;
use sentinel::source::VortexSource;
use sentinel::store::Store;
use std::sync::Arc;
use tokio::sync::broadcast;
use vortex::events::logs::Invocation;
use axum::{routing::post, Json, Router};
use base64::Engine;
use serde_json::{json, Value};
use vortex::events::{AccountRef, Instruction, TxError, VortexTransaction};
use vortex::hub::HubStats;

const PROGRAM: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";
const LOADER: &str = "BPFLoaderUpgradeab1e11111111111111111111111";
const AUTHORITY: &str = "UpgradeAuthority1111111111111111111111111";
const NEW_AUTHORITY: &str = "NewAuthority11111111111111111111111111111";

struct FakeSource(broadcast::Sender<Arc<VortexTransaction>>);

impl VortexSource for FakeSource {
    fn subscribe(&self) -> broadcast::Receiver<Arc<VortexTransaction>> {
        self.0.subscribe()
    }
    fn watch_programs(&self, _: Vec<String>) {}
    fn health(&self) -> HubStats {
        HubStats {
            transactions: 0,
            last_slot: 0,
            last_transaction_at: None,
            started_at: Utc::now(),
            programs: vec![],
            subscribers: 0,
        }
    }
}

fn account(pubkey: &str, signer: bool) -> AccountRef {
    AccountRef { pubkey: pubkey.into(), signer, writable: true, from_lookup_table: false, pre_lamports: 10, post_lamports: 5 }
}

/// Ordinary program traffic.
fn traffic(n: u64, second: i64, ok: bool) -> Arc<VortexTransaction> {
    Arc::new(VortexTransaction {
        signature: format!("sig{n}"),
        slot: 1000 + n,
        index: 0,
        received_at: Utc.timestamp_opt(second, 0).unwrap(),
        success: ok,
        error: (!ok).then(|| TxError {
            message: "InstructionError(2, Custom(6003))".into(),
            instruction_index: Some(2),
            custom_code: Some(6003),
            program_id: Some(PROGRAM.into()),
            name: Some("TooLittleSolReceived".into()),
            class: "Unknown".into(),
        }),
        fee: 5000,
        compute_units: Some(40_000),
        compute_unit_limit: None,
        compute_unit_price: None,
        accounts: vec![account(&format!("Payer{:0>39}", n % 50), true), account(PROGRAM, false)],
        instructions: vec![],
        invocations: vec![Invocation {
            program_id: PROGRAM.into(),
            depth: 1,
            instruction: Some("Sell".into()),
            compute_consumed: Some(40_000),
            success: Some(ok),
            failure: (!ok).then(|| "custom program error: 0x1773".into()),
            ..Default::default()
        }],
        logs: vec![],
        logs_truncated: false,
        token_balances: vec![],
        transfers: vec![],
        filters: vec![],
    })
}


const SQUADS: &str = "SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf";
const MULTISIG: &str = "Mu1tisig11111111111111111111111111111111111";
const VAULT: &str = "VauLtPda1111111111111111111111111111111111";
const MEMBER: &str = "Member11111111111111111111111111111111111";

fn discriminator(name: &str) -> Vec<u8> {
    solana_sdk::hash::hashv(&[b"global:", name.as_bytes()]).to_bytes()[..8].to_vec()
}

/// A Squads `vault_transaction_execute` whose vault then upgrades the program through the loader.
fn squads_upgrade(signature: &str, second: i64) -> Arc<VortexTransaction> {
    let pd = programdata_address(PROGRAM).unwrap();
    let mut tx = (*traffic(0, second, true)).clone();
    tx.signature = signature.into();
    tx.invocations = vec![];
    tx.accounts = [MEMBER, MULTISIG, VAULT, &pd, PROGRAM, "Buffer11111111111111111111111111111111111", "Proposal111111111111111111111111111111111", "Transaction11111111111111111111111111111111"]
        .iter()
        .map(|a| account(a, *a == MEMBER))
        .collect();
    let b64 = |d: Vec<u8>| base64::engine::general_purpose::STANDARD.encode(d);
    let ix = |path: &str, program: &str, accounts: Vec<&str>, data: Vec<u8>, inner: Option<u16>| Instruction {
        path: path.into(),
        top_index: 0,
        inner_index: inner,
        stack_height: 1 + inner.is_some() as u32,
        program_id: program.into(),
        program_name: None,
        accounts: accounts.into_iter().map(String::from).collect(),
        data: b64(data),
        name: None,
        parsed: None,
    };
    tx.instructions = vec![
        ix("0", SQUADS, vec![MULTISIG, "Proposal111111111111111111111111111111111", "Transaction11111111111111111111111111111111", MEMBER], discriminator("vault_transaction_execute"), None),
        // The loader's Upgrade: [programdata, program, buffer, spill, rent, clock, authority].
        ix("0.0", LOADER, vec![&pd, PROGRAM, "Buffer11111111111111111111111111111111111", "Spill111111111111111111111111111111111111", "SysvarRent111111111111111111111111111111111", "SysvarC1ock11111111111111111111111111111111", VAULT], 3u32.to_le_bytes().to_vec(), Some(0)),
    ];
    Arc::new(tx)
}

/// A Squads v4 Multisig account holding 3 signatures out of 5 members.
fn multisig_account() -> Vec<u8> {
    let mut d = vec![0u8; 8 + 64];
    d.extend(3u16.to_le_bytes());
    d.extend(0u32.to_le_bytes());
    d.extend(9u64.to_le_bytes());
    d.extend(4u64.to_le_bytes());
    d.push(0);
    d.push(253);
    d.extend(5u32.to_le_bytes());
    for i in 0..5u8 {
        d.extend([i + 1; 32]);
        d.push(7);
    }
    d
}

async fn mock_rpc() -> String {
    let data = base64::engine::general_purpose::STANDARD.encode(multisig_account());
    let app = Router::new().route(
        "/",
        post(move |Json(req): Json<Value>| {
            let data = data.clone();
            async move {
                let result = match req["method"].as_str().unwrap_or_default() {
                    "getVersion" => json!({ "solana-core": "1.18.26", "feature-set": 1 }),
                    "getAccountInfo" => json!({
                        "context": { "slot": 1 },
                        "value": { "data": [data, "base64"], "executable": false, "lamports": 1, "owner": SQUADS, "rentEpoch": 0, "space": 400 }
                    }),
                    _ => Value::Null,
                };
                Json(json!({ "jsonrpc": "2.0", "id": req["id"].clone(), "result": result }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

#[tokio::test]
async fn an_upgrade_a_squads_multisig_executed_names_it_and_what_it_requires() {
    let url = mock_rpc().await;
    let dir = std::env::temp_dir().join(format!("sentinel-squads-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    let store = Arc::new(Store::open(dir.to_str().unwrap()).unwrap());
    let (bus, _) = broadcast::channel(16);
    let rpc = Arc::new(solana_client::nonblocking::rpc_client::RpcClient::new(url));
    let s = Sentinel::new(store.clone(), Arc::new(FakeSource(bus)), Some(rpc), sentinel::pricing::PriceBook::new(), "http://ui".into()).unwrap();
    s.add_program(PROGRAM.into(), None).unwrap();

    s.on_transaction(squads_upgrade("squadsUpgradeSig", 1_700_000_000));
    s.on_tick(1_700_000_001);
    let incident = store
        .incidents(Some(PROGRAM), 5)
        .unwrap()
        .into_iter()
        .find(|i| i.kind == IncidentKind::AuthorityChange)
        .expect("upgrade incident");
    let a = &incident.evidence["authority"];
    assert_eq!(a["action"], "upgrade");
    assert_eq!(a["via"]["program"], "Squads v4");
    assert_eq!(a["via"]["multisig"], MULTISIG);
    assert_eq!(a["via"]["executor"], MEMBER, "the member that signed it");
    assert_eq!(a["via"]["instruction"], "vault_transaction_execute");
    assert!(incident.summary.contains("Squads v4 multisig Mu1t"), "{}", incident.summary);
    assert!(!incident.summary.contains("via CPI"), "the vague wording is replaced by the real name");

    // Reading the multisig adds how many signatures it takes.
    s.enrich_multisig(incident.id, MULTISIG).await.unwrap();
    s.on_tick(1_700_000_002); // an open incident is saved on the next tick
    let enriched = store.incident(incident.id).unwrap().unwrap();
    assert_eq!(enriched.evidence["authority"]["multisig"]["threshold"], 3);
    assert_eq!(enriched.evidence["authority"]["multisig"]["members"], 5);
    assert!(enriched.summary.contains("3 of 5 signatures"), "{}", enriched.summary);
    // Doing it again doesn't repeat itself.
    s.enrich_multisig(incident.id, MULTISIG).await.unwrap();
    s.on_tick(1_700_000_003);
    let again = store.incident(incident.id).unwrap().unwrap();
    assert_eq!(again.summary.matches("3 of 5").count(), 1);

    // An upgrade through a plain key is still described as before.
    let direct = serde_json::from_value::<Incident>(json!({
        "id": 0, "program_id": PROGRAM, "kind": "authority_change", "severity": "high", "status": "open",
        "title": "t", "summary": "s", "explanation": "e", "source": "detector", "metric": null, "observed": null,
        "peak": null, "baseline": null, "threshold": null, "onset_at": null, "detected_at": "2024-01-01T00:00:00Z",
        "updated_at": "2024-01-01T00:00:00Z", "resolved_at": null, "detection_latency_ms": null,
        "affected_count": 0, "affected_wallets": 0, "evidence": {}
    }))
    .unwrap();
    assert!(direct.evidence["authority"]["via"].is_null());
    let _ = std::fs::remove_file(&dir);
}
