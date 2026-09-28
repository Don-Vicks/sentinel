//! Sentinel service: starts the Vortex Geyser stream (Solami Yellowstone
//! gRPC), wires it to the Sentinel engine and serves the API + dashboard.

use anyhow::Result;
use sentinel::engine::Sentinel;
use sentinel::source::VortexSource;
use sentinel::store::Store;
use solana_client::nonblocking::rpc_client::RpcClient;
use std::env;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use vortex::geyser::GeyserEvent;
use vortex::hub::VortexHub;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,h2=warn,hyper=warn,tower=warn".into()),
        )
        .init();

    let port: u16 = env::var("SENTINEL_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8080);
    let db_path = env::var("SENTINEL_DB").unwrap_or_else(|_| "sentinel.db".into());
    let public_url =
        env::var("SENTINEL_PUBLIC_URL").unwrap_or_else(|_| format!("http://localhost:{port}"));
    let web_dir = env::var("SENTINEL_WEB_DIR").unwrap_or_else(|_| "web/dist".into());

    // --- Vortex: Geyser stream -> hub ---------------------------------------
    let hub = VortexHub::new(None);
    let simulate = env::var("SENTINEL_SIMULATE").ok().filter(|p| !p.is_empty());
    if let Some(program) = &simulate {
        sentinel::simulate::spawn(hub.clone(), program.clone());
    }
    let (event_tx, mut event_rx) = mpsc::channel::<GeyserEvent>(20_000);
    let filters = hub.stream_filters();
    tokio::spawn(async move {
        if simulate.is_some() {
            return;
        }
        loop {
            match vortex::geyser::client::connect().await {
                Ok(client) => {
                    if let Err(e) =
                        vortex::geyser::stream::subscribe(client, event_tx.clone(), filters.clone())
                            .await
                    {
                        tracing::error!(error = %e, "Vortex Geyser stream stopped");
                    } else {
                        return;
                    }
                }
                Err(e) => tracing::error!(error = %e, "Vortex could not connect to Yellowstone"),
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
    let hub_fwd = hub.clone();
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            match event {
                GeyserEvent::Slot(s) => hub_fwd.record_slot(s.slot),
                GeyserEvent::Transaction(tx) => hub_fwd.publish(tx),
                GeyserEvent::Tx(_) => {}
            }
        }
    });

    // --- Sentinel -------------------------------------------------------------
    let rpc = env::var("SOLANA_RPC_URL")
        .ok()
        .filter(|u| !u.is_empty())
        .map(|u| Arc::new(RpcClient::new(u)));
    if rpc.is_none() {
        tracing::warn!("SOLANA_RPC_URL not set; account owner labels in traces are disabled");
    }
    let store = Arc::new(Store::open(&db_path)?);
    let source: Arc<dyn VortexSource> = hub.clone();
    let sentinel = Sentinel::new(store, source, rpc, public_url.clone())?;

    for id in env::var("SENTINEL_PROGRAMS").unwrap_or_default().split(',') {
        let id = id.trim();
        if !id.is_empty() {
            match sentinel.add_program(id.to_string(), None) {
                Ok(p) => tracing::info!(program = %p.program_id, label = %p.label, "monitoring"),
                Err(e) => tracing::warn!(program = id, error = %e, "skipping program"),
            }
        }
    }
    tokio::spawn(sentinel.clone().run());

    // --- HTTP -----------------------------------------------------------------
    let web = ServeDir::new(&web_dir).fallback(ServeFile::new(format!("{web_dir}/index.html")));
    let app = sentinel::api::router(sentinel)
        .fallback_service(web)
        .layer(CorsLayer::permissive());
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!("Vortex Sentinel on {public_url}");
    axum::serve(listener, app).await?;
    Ok(())
}
