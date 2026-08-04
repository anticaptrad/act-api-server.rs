use std::{net::SocketAddr, sync::Arc};

use tokio::signal;

use crate::{config, nats, routes, telemetry, youtube};

/// Initialize configuration, observability, fail-soft dependencies, and HTTP.
pub(crate) async fn run() -> anyhow::Result<()> {
    let cfg = config::Config::from_env()?;
    telemetry::init(&cfg.service_name)?;
    let _telemetry = TelemetryGuard;

    serve(cfg).await?;
    tracing::info!("shutdown complete");
    Ok(())
}

async fn serve(cfg: config::Config) -> anyhow::Result<()> {
    let nats = nats::connect(&cfg.nats_url).await;
    if let Some(client) = nats.clone() {
        nats::spawn_event_subscriber(client);
    }

    let youtube = cfg
        .youtube
        .as_ref()
        .map(youtube::YoutubeGasClient::new)
        .transpose()?;

    tracing::info!(
        youtube_configured = youtube.is_some(),
        admin_auth_configured = cfg.admin_api_key.is_some(),
        "control-plane configuration loaded"
    );

    let app = routes::router(routes::AppState {
        nats,
        youtube,
        admin_api_key: cfg.admin_api_key.map(Arc::<str>::from),
    });

    let address = bind_address(cfg.port);
    let listener = tokio::net::TcpListener::bind(address).await?;
    tracing::info!(%address, service = %cfg.service_name, "act-api-server listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

fn bind_address(port: u16) -> SocketAddr {
    SocketAddr::from(([0, 0, 0, 0], port))
}

/// Flush buffered OTLP spans on every return path after telemetry initializes.
struct TelemetryGuard;

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        telemetry::shutdown();
    }
}

/// Resolve when the process receives SIGINT (Ctrl-C) or SIGTERM (k8s pod stop),
/// enabling axum's graceful shutdown to drain in-flight requests.
async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl-C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("shutdown signal received");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_address_uses_all_ipv4_interfaces_and_configured_port() {
        assert_eq!(bind_address(8080), "0.0.0.0:8080".parse().unwrap());
        assert_eq!(bind_address(9124), "0.0.0.0:9124".parse().unwrap());
    }
}
