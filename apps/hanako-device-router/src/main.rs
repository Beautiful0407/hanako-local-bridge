use std::{env, net::SocketAddr, path::PathBuf, str::FromStr, sync::Arc, time::Duration};

use hanako_device_router::{RouterPaths, RouterService};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = env::var_os("HANA_DEVICE_ROUTER_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("devices.json"));
    let base = config.parent().unwrap_or_else(|| std::path::Path::new("."));
    let paths = RouterPaths {
        config: config.clone(),
        cache: env::var_os("HANA_DEVICE_ROUTER_CACHE")
            .map(PathBuf::from)
            .unwrap_or_else(|| base.join("tools-cache.json")),
        queue: env::var_os("HANA_DEVICE_ROUTER_QUEUE")
            .map(PathBuf::from)
            .unwrap_or_else(|| base.join("offline-queue.json")),
    };
    let host = env::var("HANA_DEVICE_ROUTER_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = env::var("HANA_DEVICE_ROUTER_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(18786);
    let interval = env::var("HANA_DEVICE_HEALTH_INTERVAL_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(10_000)
        .max(2_000);
    let service = Arc::new(RouterService::load(paths, Duration::from_millis(interval)).await?);
    service.refresh_all().await;
    service.refresh_tools().await;
    service.clone().start_background_refresh();
    let address = SocketAddr::from_str(&format!("{host}:{port}"))?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    println!(
        "[device-router] v{} listening on http://{address}/mcp",
        env!("CARGO_PKG_VERSION")
    );
    axum::serve(listener, service.router()).await?;
    Ok(())
}
