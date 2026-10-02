//! Prints the on-chain Anchor IDL name of each program in a TSV (id, label, ...), to check that a
//! label matches what the program calls itself: `cargo run --example idl_names -- programs.tsv`.
use sentinel::idl::IdlRegistry;
use solana_client::nonblocking::rpc_client::RpcClient;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv::dotenv().ok();
    let path = std::env::args().nth(1).expect("path to a TSV of id<TAB>label");
    let rpc = Arc::new(RpcClient::new(std::env::var("SOLANA_RPC_URL")?));
    let idls = IdlRegistry::new(Some(rpc));
    for line in std::fs::read_to_string(path)?.lines().filter(|l| !l.trim().is_empty()) {
        let mut cols = line.split('\t');
        let (id, label) = (cols.next().unwrap(), cols.next().unwrap_or(""));
        idls.request(id);
        let name = idls.get(id).await.and_then(|i| i.name.clone());
        println!("{:<26} {:<40} IDL: {}", label, id, name.as_deref().unwrap_or("none (not Anchor, or no IDL published)"));
    }
    Ok(())
}
