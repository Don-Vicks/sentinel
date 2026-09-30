//! Resolves an address or signature against a live RPC: `cargo run --example resolve -- <input>`.
use sentinel::resolve::*;
use solana_client::nonblocking::rpc_client::RpcClient;
use std::str::FromStr;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let url = std::env::var("RPC_URL").unwrap_or("https://api.mainnet-beta.solana.com".into());
    let rpc = RpcClient::new(url);
    let input = std::env::args().nth(1).expect("input");
    match parse_query(&input)? {
        Query::Address(a) => println!("{:#?}", resolve_address(&rpc, &a).await?),
        Query::Signature(s) => println!("signature {s}"),
    }
    let _ = solana_sdk::pubkey::Pubkey::from_str(UPGRADEABLE_LOADER)?;
    Ok(())
}
