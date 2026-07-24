//! HTTP surface: liveness and readiness probes consumed by k8s.

use axum::{Json, Router, extract::State, routing::get};
use serde_json::{Value, json};

#[derive(Clone)]
pub struct AppState {
    pub nats: Option<async_nats::Client>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .with_state(state)
}

/// Liveness: the process is up and the event loop is responsive.
async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

/// Readiness: the process can serve traffic. NATS is an optional dependency, so
/// its state is reported for observability but does not gate readiness.
async fn ready(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "ready": true,
        "nats_connected": state.nats.is_some(),
    }))
}
