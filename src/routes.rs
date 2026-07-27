//! HTTP surface for Kubernetes probes and the guarded YouTube control plane.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header::WWW_AUTHENTICATE},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    auth::{AuthFailure, require_bearer},
    nats,
    youtube::{YoutubeAction, YoutubeClientError, YoutubeGasClient, redact_map_for_audit},
};

const MAX_CONTROL_BODY_BYTES: usize = 1024 * 1024;
static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub struct AppState {
    pub nats: Option<async_nats::Client>,
    pub youtube: Option<YoutubeGasClient>,
    pub admin_api_key: Option<Arc<str>>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/v1/youtube/health", get(youtube_health))
        .route("/v1/youtube/status", get(youtube_status))
        .route("/v1/youtube/actions/:action", post(youtube_action))
        .layer(DefaultBodyLimit::max(MAX_CONTROL_BODY_BYTES))
        .with_state(state)
}

/// Liveness: the process is up and the event loop is responsive.
async fn health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "act-api-server",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

/// Readiness: the core HTTP process is serving. NATS and Apps Script are
/// fail-soft dependencies whose configuration is reported without exposing
/// endpoints or credentials.
async fn ready(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "ready": true,
        "nats_connected": state.nats.is_some(),
        "youtube_configured": state.youtube.is_some(),
        "admin_auth_configured": state.admin_api_key.is_some(),
    }))
}

/// Calls the unauthenticated Apps Script `health` action. This deliberately
/// exercises DNS, TLS, redirect handling, deployment access, and JSON parsing.
async fn youtube_health(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let request_id = request_id();
    let client = state
        .youtube
        .as_ref()
        .ok_or_else(ApiError::youtube_not_configured)?;
    let started = Instant::now();
    match client.health().await {
        Ok(upstream) => Ok(Json(json!({
            "ok": true,
            "requestId": request_id,
            "durationMs": started.elapsed().as_millis(),
            "data": upstream,
        }))),
        Err(error) => {
            tracing::warn!(%request_id, %error, "Apps Script health check failed");
            Err(ApiError::from_youtube(error, request_id))
        }
    }
}

async fn youtube_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    authorize(&state, &headers)?;
    let client = state
        .youtube
        .as_ref()
        .ok_or_else(ApiError::youtube_not_configured)?;
    Ok(Json(json!({
        "ok": true,
        "data": {
            "configured": true,
            "expectedChannelHandle": client.expected_channel_handle(),
            "deploymentId": client.deployment_id(),
            "publicActionsEnabled": client.allow_public_actions(),
            "appsScriptApiKeyPresent": true,
            "appsScriptApiKeyExposed": false,
        }
    })))
}

async fn youtube_action(
    State(state): State<AppState>,
    Path(action_name): Path<String>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    authorize(&state, &headers)?;
    let action = YoutubeAction::parse(&action_name).ok_or_else(|| ApiError {
        status: StatusCode::NOT_FOUND,
        code: "UNKNOWN_YOUTUBE_ACTION",
        message: format!("unsupported YouTube action: {action_name}"),
        details: None,
        request_id: None,
    })?;
    let idempotency_key = header_string(&headers, "idempotency-key")?;
    let request_id = idempotency_key
        .as_deref()
        .map(ToOwned::to_owned)
        .unwrap_or_else(request_id);
    let client = state
        .youtube
        .as_ref()
        .ok_or_else(ApiError::youtube_not_configured)?;
    let audit_fields = redact_map_for_audit(&payload);
    let started = Instant::now();

    nats::publish_json(
        state.nats.as_ref(),
        &format!("act.youtube.{}.requested", action.as_str()),
        &json!({
            "requestId": request_id,
            "action": action.as_str(),
            "mutating": action.is_mutating(),
            "fields": audit_fields,
        }),
    )
    .await;

    match client
        .execute(action, payload, idempotency_key.as_deref())
        .await
    {
        Ok(data) => {
            let duration_ms = started.elapsed().as_millis();
            nats::publish_json(
                state.nats.as_ref(),
                &format!("act.youtube.{}.succeeded", action.as_str()),
                &json!({
                    "requestId": request_id,
                    "action": action.as_str(),
                    "durationMs": duration_ms,
                }),
            )
            .await;
            Ok(Json(json!({
                "ok": true,
                "requestId": request_id,
                "durationMs": duration_ms,
                "data": data,
            })))
        }
        Err(error) => {
            let duration_ms = started.elapsed().as_millis();
            tracing::warn!(
                %request_id,
                action = action.as_str(),
                %duration_ms,
                code = error.code(),
                error = %error,
                "Apps Script YouTube action failed"
            );
            nats::publish_json(
                state.nats.as_ref(),
                &format!("act.youtube.{}.failed", action.as_str()),
                &json!({
                    "requestId": request_id,
                    "action": action.as_str(),
                    "durationMs": duration_ms,
                    "errorCode": error.code(),
                }),
            )
            .await;
            Err(ApiError::from_youtube(error, request_id))
        }
    }
}

fn authorize(state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    require_bearer(headers, state.admin_api_key.as_deref()).map_err(|failure| match failure {
        AuthFailure::NotConfigured => ApiError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "ADMIN_AUTH_NOT_CONFIGURED",
            message: "ADMIN_API_KEY is not configured; administrative routes are closed"
                .to_string(),
            details: None,
            request_id: None,
        },
        AuthFailure::Missing | AuthFailure::Invalid => ApiError {
            status: StatusCode::UNAUTHORIZED,
            code: "UNAUTHORIZED",
            message: "a valid Authorization: Bearer credential is required".to_string(),
            details: None,
            request_id: None,
        },
    })
}

fn header_string(headers: &HeaderMap, name: &'static str) -> Result<Option<String>, ApiError> {
    match headers.get(name) {
        None => Ok(None),
        Some(value) => value
            .to_str()
            .map(|value| Some(value.to_string()))
            .map_err(|_| ApiError {
                status: StatusCode::BAD_REQUEST,
                code: "INVALID_HEADER",
                message: format!("{name} must contain valid ASCII"),
                details: None,
                request_id: None,
            }),
    }
}

fn request_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let sequence = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("yt-{millis}-{sequence}")
}

#[derive(Debug, Serialize)]
struct ErrorBody<'a> {
    ok: bool,
    error: ErrorDescription<'a>,
    #[serde(rename = "requestId", skip_serializing_if = "Option::is_none")]
    request_id: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct ErrorDescription<'a> {
    code: &'a str,
    message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<&'a Value>,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    details: Option<Value>,
    request_id: Option<String>,
}

impl ApiError {
    fn youtube_not_configured() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "YOUTUBE_GAS_NOT_CONFIGURED",
            message: "set YOUTUBE_GAS_URL and YOUTUBE_GAS_API_KEY to enable channel management"
                .to_string(),
            details: None,
            request_id: None,
        }
    }

    fn from_youtube(error: YoutubeClientError, request_id: String) -> Self {
        let status = match error {
            YoutubeClientError::Validation(_) => StatusCode::BAD_REQUEST,
            _ => StatusCode::BAD_GATEWAY,
        };
        Self {
            status,
            code: error.code(),
            message: error.to_string(),
            details: error.details(),
            request_id: Some(request_id),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorBody {
            ok: false,
            error: ErrorDescription {
                code: self.code,
                message: &self.message,
                details: self.details.as_ref(),
            },
            request_id: self.request_id.as_deref(),
        };
        let mut response = (self.status, Json(body)).into_response();
        if self.status == StatusCode::UNAUTHORIZED {
            response.headers_mut().insert(
                WWW_AUTHENTICATE,
                HeaderValue::from_static("Bearer realm=\"anticaptrad-admin\""),
            );
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, header::AUTHORIZATION};

    use super::{AppState, authorize, request_id};

    #[test]
    fn administrative_routes_fail_closed() {
        let state = AppState {
            nats: None,
            youtube: None,
            admin_api_key: None,
        };
        assert!(authorize(&state, &HeaderMap::new()).is_err());
    }

    #[test]
    fn administrative_routes_accept_configured_key() {
        let state = AppState {
            nats: None,
            youtube: None,
            admin_api_key: Some("a-very-long-administrative-api-key".into()),
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer a-very-long-administrative-api-key"),
        );
        assert!(authorize(&state, &headers).is_ok());
    }

    #[test]
    fn request_ids_are_unique() {
        assert_ne!(request_id(), request_id());
    }
}
