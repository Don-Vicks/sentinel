//! Controlled demo traffic on mainnet. Sends Memo transactions from a canary
//! wallet through your RPC (Solami). `--fail` sends memos with invalid UTF-8,
//! which land on chain and fail inside the Memo program, so Sentinel sees a
//! real failure spike through the normal Vortex stream.
//!
//!     cargo run --example canary -- --count 8 --fail
//!
//! Monitor the canary wallet's address in Sentinel and add a rule such as
//! "failed transactions > 3 over 60s". Each transaction costs the base fee
//! (5,000 lamports) plus the optional priority fee.

use anyhow::{Context, Result};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_config::RpcSendTransactionConfig;
use solana_sdk::compute_budget::ComputeBudgetInstruction;
use solana_sdk::instruction::Instruction;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{read_keypair_file, Signer};
use solana_sdk::transaction::Transaction;
use std::str::FromStr;

const MEMO_PROGRAM: &str = "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr";

#[tokio::main]
async fn main() -> Result<()> {
    dotenv::dotenv().ok();
    let args: Vec<String> = std::env::args().collect();
    let fail = args.iter().any(|a| a == "--fail");
    let count: usize = args
        .iter()
        .position(|a| a == "--count")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);

    let rpc_url = std::env::var("SOLANA_RPC_URL").context("SOLANA_RPC_URL must be set")?;
    let key_path = std::env::var("CANARY_KEYPAIR").unwrap_or_else(|_| "canary-keypair.json".into());
    let payer = read_keypair_file(&key_path)
        .map_err(|e| anyhow::anyhow!("reading {key_path}: {e}"))?;
    let rpc = RpcClient::new(rpc_url);
    let memo = Pubkey::from_str(MEMO_PROGRAM)?;

    println!("canary wallet {} (monitor this address in Sentinel)", payer.pubkey());
    for i in 0..count {
        let data = if fail {
            // Invalid UTF-8: the Memo program rejects it at execution time.
            vec![0xff, 0xfe, 0xfd, i as u8]
        } else {
            format!("sentinel canary {i}").into_bytes()
        };
        let blockhash = rpc.get_latest_blockhash().await?;
        let tx = Transaction::new_signed_with_payer(
            &[
                ComputeBudgetInstruction::set_compute_unit_price(10_000),
                Instruction::new_with_bytes(memo, &data, vec![]),
            ],
            Some(&payer.pubkey()),
            &[&payer],
            blockhash,
        );
        // Skip preflight so failing transactions still land (and show up).
        let sig = rpc
            .send_transaction_with_config(
                &tx,
                RpcSendTransactionConfig {
                    skip_preflight: true,
                    ..Default::default()
                },
            )
            .await?;
        println!("sent {} {sig}", if fail { "failing" } else { "ok" });
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    }
    Ok(())
}
