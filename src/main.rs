//! act-api-server — HTTP API + NATS event-bus bridge for the AntiCapTrad platform.
//!
//! Deployed to the k8s cluster at `~/codes/ores/k8s-cluster` alongside the
//! shared NATS bridge and OpenTelemetry collector.

mod auth;
mod config;
mod nats;
mod routes;
mod server;
mod telemetry;
mod youtube;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    server::run().await
}
