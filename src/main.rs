//! act-api-server — HTTP API + NATS event-bus bridge for the AntiCapTrad platform.
//!
//! Deployed to the k8s cluster at `~/codes/ores/k8s-cluster` alongside the
//! shared NATS bridge and OpenTelemetry collector.

mod auth;
mod config;
mod flags;
mod nats;
mod routes;
mod server;
mod telemetry;
mod youtube;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if let Some(output) = flags::process_control().map_err(std::io::Error::other)? {
        print!("{output}");
        return Ok(());
    }
    server::run().await
}
