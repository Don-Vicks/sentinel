//! Prints the Anchor IDL a program keeps on chain: `cargo run --example fetch_idl -- <program> [rpc-url]`.
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let program = args.next().expect("a program address");
    let url = args.next().unwrap_or_else(|| "https://api.mainnet-beta.solana.com".into());
    let rpc = RpcClient::new(url);
    let data = rpc.get_account_data(&sentinel::idl::idl_address(&Pubkey::from_str(&program)?)).await?;
    println!("{}", serde_json::to_string_pretty(&sentinel::idl::decode_idl_account(&data)?)?);
    Ok(())
}
