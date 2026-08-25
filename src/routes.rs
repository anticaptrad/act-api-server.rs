//! HTTP surface for Kubernetes probes and the guarded YouTube control plane.

use std::{
    sync::Arc,
    sync::atomic::{AtomicU64, Ordering},
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

use act_api_server::transport_runtime::OperationJournal;

use crate::{
    auth::{AuthFailure, AuthSubject, SharedAuthVerifier},
    nats,
    youtube::{YoutubeAction, YoutubeClientError, YoutubeGasClient, redact_map_for_audit},
};

const MAX_CONTROL_BODY_BYTES: usize = 1024 * 1024;
const YOUTUBE_ADMIN_SCOPES: [&str; 1] = ["youtube:admin"];
static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub struct AppState {
    pub nats: Option<async_nats::Client>,
    pub youtube: Option<YoutubeGasClient>,
    pub shared_auth: Option<SharedAuthVerifier>,
    pub operation_journal: Option<Arc<dyn OperationJournal>>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/v1/youtube/health", get(youtube_health))
        .route("/v1/youtube/status", get(youtube_status))
        .route("/v1/operations/:operation_id", get(operation_status))
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
        "shared_auth_configured": state.shared_auth.is_some(),
        "durable_operations_configured": state.operation_journal.is_some(),
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
    authorize(&state, &headers).await?;
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
    authorize(&state, &headers).await?;
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

async fn operation_status(
    State(state): State<AppState>,
    Path(operation_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let actor = authorize(&state, &headers).await?;
    if operation_id.is_empty()
        || operation_id.len() > 128
        || operation_id.trim() != operation_id
        || operation_id.chars().any(char::is_control)
    {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            code: "INVALID_OPERATION_ID",
            message: "operation id is invalid".to_string(),
            details: None,
            request_id: None,
        });
    }
    let journal = state.operation_journal.as_ref().ok_or_else(|| ApiError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        code: "DURABLE_OPERATIONS_NOT_CONFIGURED",
        message: "durable operation status is unavailable".to_string(),
        details: None,
        request_id: None,
    })?;
    let status = journal
        .status(&operation_id, &actor.subject)
        .await
        .map_err(|_| ApiError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "DURABLE_OPERATION_STATUS_UNAVAILABLE",
            message: "durable operation status is unavailable".to_string(),
            details: None,
            request_id: None,
        })?
        .ok_or_else(|| ApiError {
            status: StatusCode::NOT_FOUND,
            code: "OPERATION_NOT_FOUND",
            message: "operation not found".to_string(),
            details: None,
            request_id: None,
        })?;
    Ok(Json(json!({"ok": true, "data": status})))
}

async fn authorize(state: &AppState, headers: &HeaderMap) -> Result<AuthSubject, ApiError> {
    let verifier = state.shared_auth.as_ref().ok_or_else(|| ApiError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        code: "SHARED_AUTH_NOT_CONFIGURED",
        message: "Shared Auth is not configured; protected routes are closed".to_string(),
        details: None,
        request_id: None,
    })?;
    verifier
        .verify(headers, &YOUTUBE_ADMIN_SCOPES)
        .await
        .map_err(|failure| match failure {
            AuthFailure::Missing | AuthFailure::Invalid => ApiError {
                status: StatusCode::UNAUTHORIZED,
                code: "UNAUTHORIZED",
                message: "a valid delegated Shared Auth bearer is required".to_string(),
                details: None,
                request_id: None,
            },
            AuthFailure::Unavailable => ApiError {
                status: StatusCode::SERVICE_UNAVAILABLE,
                code: "SHARED_AUTH_UNAVAILABLE",
                message: "Shared Auth verification is unavailable".to_string(),
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
        let status = match &error {
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
    use axum::http::HeaderMap;

    use super::{AppState, authorize, request_id};

    #[tokio::test]
    async fn administrative_routes_fail_closed() {
        let state = AppState {
            nats: None,
            youtube: None,
            shared_auth: None,
            operation_journal: None,
        };
        assert!(authorize(&state, &HeaderMap::new()).await.is_err());
    }

    #[test]
    fn request_ids_are_unique() {
        assert_ne!(request_id(), request_id());
    }
}
