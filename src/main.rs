//! act-api-server — HTTP API + NATS event-bus bridge for the AntiCapTrad platform.
//!
//! Deployed to the k8s cluster at ~/codes/ores/k8s-cluster alongside the shared
//! NATS bridge and the OpenTelemetry collector.

mod config;
mod nats;
mod routes;
mod telemetry;

use std::net::SocketAddr;
use tokio::signal;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = config::Config::from_env();
    telemetry::init(&cfg.service_name)?;

    let nats = nats::connect(&cfg.nats_url).await;
    if let Some(client) = nats.clone() {
        nats::spawn_event_subscriber(client);
    }

    let app = routes::router(routes::AppState { nats });

    let addr = SocketAddr::from(([0, 0, 0, 0], cfg.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, service = %cfg.service_name, "act-api-server listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    tracing::info!("shutdown complete");
    telemetry::shutdown();
    Ok(())
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
