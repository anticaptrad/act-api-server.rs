//! Runtime configuration sourced entirely from the environment.
//!
//! Secrets and endpoints are injected by the k8s deployment (see act-infra).
//! No `.env` files are used — the `dotenv` crate is blacklisted across all repos
//! (see `agents.md`).

#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub nats_url: String,
    pub service_name: String,
}

impl Config {
    pub fn from_env() -> Self {
        let port = std::env::var("PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(8080);

        let nats_url =
            std::env::var("NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".to_string());

        let service_name =
            std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| "act-api-server".to_string());

        Self {
            port,
            nats_url,
            service_name,
        }
    }
}
