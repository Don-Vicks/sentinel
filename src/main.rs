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

    // SENTINEL_PORT wins; hosting platforms such as Railway provide PORT.
    let port: u16 = env::var("SENTINEL_PORT")
        .or_else(|_| env::var("PORT"))
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
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
    let simulating = simulate.is_some();
    // Mirage (the same Yellowstone frames over a WebSocket) is the failover when gRPC can't
    // connect. Sentinel sets the subscription up and keeps it in step with the watchlist itself.
    let mirage = sentinel::mirage_setup::handle().clone();
    if !simulating {
        tokio::spawn(sentinel::mirage_setup::run(filters.clone()));
    }
    let mirage_first = env::var("SENTINEL_TRANSPORT").is_ok_and(|t| t == "mirage");
    let stream_hub = hub.clone();
    tokio::spawn(async move {
        if simulating {
            return;
        }
        // gRPC failures in a row; a stream that exhausts its own retries counts as several.
        let mut failures = 0u32;
        loop {
            if !mirage_first {
                match vortex::geyser::client::connect().await {
                    Ok(client) => {
                        match vortex::geyser::stream::subscribe(client, event_tx.clone(), filters.clone()).await {
                            Ok(()) => return,
                            Err(e) => {
                                tracing::error!(error = %e, "Vortex Geyser stream stopped");
                                failures += 3;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Vortex could not connect to Yellowstone");
                        failures += 1;
                    }
                }
            }
            let url = if mirage_first {
                mirage.wait_url(Duration::from_secs(30)).await
            } else if failures >= 3 {
                mirage.url()
            } else {
                None
            };
            if let Some(url) = url.as_deref() {
                tracing::warn!("Solami gRPC unavailable; streaming through Mirage");
                stream_hub.set_transport("mirage");
                let began = std::time::Instant::now();
                // Retry gRPC every couple of minutes; stay on Mirage for good if it was chosen.
                while mirage_first || began.elapsed() < Duration::from_secs(120) {
                    match vortex::geyser::mirage::session(url, &event_tx).await {
                        Ok(true) => return,
                        Ok(false) => {}
                        Err(e) => tracing::error!(error = %e, "Mirage stream error"),
                    }
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                stream_hub.set_transport("grpc");
                failures = 0;
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
    // Solami Blur prices transfers in USD. The same Solami key works if it
    // carries the DataApi permission.
    let prices = sentinel::pricing::PriceBook::new();
    let blur_key = env::var("BLUR_API_KEY")
        .or_else(|_| env::var("YELLOWSTONE_TOKEN"))
        .ok()
        .filter(|k| !k.is_empty());
    match blur_key {
        Some(key) if simulate.is_none() => {
            let url = env::var("BLUR_API_URL").unwrap_or_else(|_| "https://api.solami.dev".into());
            prices.spawn(url, key);
        }
        _ => tracing::warn!("Blur pricing disabled (no BLUR_API_KEY); transfers show no USD values"),
    }
    if simulate.is_some() {
        // Fixed demo prices so USD values render without a key.
        prices.insert(sentinel::pricing::WSOL, 180.0, f64::INFINITY);
    }

    let store = Arc::new(Store::open(&db_path)?);
    // Retention: keep the database inside a budget so a small volume never fills.
    let retain_hours: i64 = env::var("SENTINEL_RETAIN_HOURS").ok().and_then(|v| v.parse().ok()).unwrap_or(48);
    let max_db_mb: i64 = env::var("SENTINEL_MAX_DB_MB").ok().and_then(|v| v.parse().ok()).unwrap_or(250);
    {
        let store = store.clone();
        tokio::task::spawn_blocking(move || loop {
            match store.prune(retain_hours, max_db_mb * 1024 * 1024) {
                Ok(n) if n > 0 => tracing::info!(removed = n, "pruned old incident data"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "prune failed"),
            }
            std::thread::sleep(std::time::Duration::from_secs(600));
        });
    }
    let source: Arc<dyn VortexSource> = hub.clone();
    let sentinel = Sentinel::new(store, source, rpc, prices, public_url.clone())?;

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
    // Client addresses feed the per-IP rate limits.
    axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await?;
    Ok(())
}
