//! Typed client and policy layer for the Anticaptrad Google Apps Script web app.
//!
//! Google ContentService responses are redirected to a one-time URL on
//! `script.googleusercontent.com`. The HTTP client follows only the documented
//! Google hosts and rejects redirects to login or arbitrary third-party hosts.

use std::{collections::HashMap, fmt};

use reqwest::{Response, StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::config::YoutubeConfig;

#[derive(Clone)]
pub struct YoutubeGasClient {
    http: reqwest::Client,
    web_app_url: Url,
    api_key: String,
    expected_channel_handle: String,
    max_response_bytes: usize,
    allow_public_actions: bool,
}

impl YoutubeGasClient {
    pub fn new(config: &YoutubeConfig) -> Result<Self, YoutubeClientError> {
        let http = reqwest::Client::builder()
            .timeout(config.timeout)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 5 {
                    return attempt.error("too many Apps Script redirects");
                }
                match attempt.url().host_str() {
                    Some("script.google.com" | "script.googleusercontent.com") => attempt.follow(),
                    _ => attempt.stop(),
                }
            }))
            .user_agent(concat!("act-api-server/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(YoutubeClientError::Http)?;

        Ok(Self {
            http,
            web_app_url: config.web_app_url.clone(),
            api_key: config.api_key.clone(),
            expected_channel_handle: config.expected_channel_handle.clone(),
            max_response_bytes: config.max_response_bytes,
            allow_public_actions: config.allow_public_actions,
        })
    }

    pub fn expected_channel_handle(&self) -> &str {
        &self.expected_channel_handle
    }

    pub fn allow_public_actions(&self) -> bool {
        self.allow_public_actions
    }

    pub fn deployment_id(&self) -> Option<&str> {
        let mut segments = self.web_app_url.path_segments()?;
        while let Some(segment) = segments.next() {
            if segment == "s" {
                return segments.next();
            }
        }
        None
    }

    pub async fn health(&self) -> Result<Value, YoutubeClientError> {
        let mut url = self.web_app_url.clone();
        url.query_pairs_mut().append_pair("action", "health");
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(YoutubeClientError::Http)?;
        self.parse_response(response).await
    }

    pub async fn execute(
        &self,
        action: YoutubeAction,
        payload: Value,
        idempotency_key: Option<&str>,
    ) -> Result<Value, YoutubeClientError> {
        let mut body =
            prepare_payload(action, payload, idempotency_key, self.allow_public_actions)?;
        body.insert(
            "action".to_string(),
            Value::String(action.as_str().to_string()),
        );
        body.insert("apiKey".to_string(), Value::String(self.api_key.clone()));

        let response = self
            .http
            .post(self.web_app_url.clone())
            .json(&body)
            .send()
            .await
            .map_err(YoutubeClientError::Http)?;
        self.parse_response(response).await
    }

    async fn parse_response(&self, response: Response) -> Result<Value, YoutubeClientError> {
        let status = response.status();
        let final_url = response.url().clone();
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);

        if response
            .content_length()
            .is_some_and(|length| length > self.max_response_bytes as u64)
        {
            return Err(YoutubeClientError::ResponseTooLarge {
                max_bytes: self.max_response_bytes,
            });
        }

        let bytes = response.bytes().await.map_err(YoutubeClientError::Http)?;
        if bytes.len() > self.max_response_bytes {
            return Err(YoutubeClientError::ResponseTooLarge {
                max_bytes: self.max_response_bytes,
            });
        }

        if !status.is_success() {
            let body = String::from_utf8_lossy(&bytes);
            let body = body.chars().take(500).collect::<String>();
            return Err(YoutubeClientError::UpstreamHttp {
                status,
                final_url,
                location,
                body,
            });
        }

        let envelope: GasEnvelope =
            serde_json::from_slice(&bytes).map_err(|source| YoutubeClientError::InvalidJson {
                source,
                preview: String::from_utf8_lossy(&bytes).chars().take(500).collect(),
            })?;

        if envelope.ok {
            Ok(envelope.data.unwrap_or(Value::Null))
        } else {
            let error = envelope.error.unwrap_or_else(|| GasError {
                message: "Apps Script returned ok=false without an error object".to_string(),
                code: "UNKNOWN_UPSTREAM_ERROR".to_string(),
                details: None,
            });
            Err(YoutubeClientError::AppsScript(error))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YoutubeAction {
    Channel,
    Videos,
    Analytics,
    ExportAnalytics,
    Jobs,
    StartUpload,
    ProcessUpload,
    ProcessAllUploads,
    PublishVideo,
    UpdateVideo,
    CreatePlaylist,
    AddToPlaylist,
    IngestGmail,
    SendDigest,
    PartnerStatus,
    PartnerOwners,
    PartnerClaims,
    AdminStatus,
    WorkspaceUsers,
}

impl YoutubeAction {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "channel" => Self::Channel,
            "videos" => Self::Videos,
            "analytics" => Self::Analytics,
            "exportAnalytics" => Self::ExportAnalytics,
            "jobs" => Self::Jobs,
            "startUpload" => Self::StartUpload,
            "processUpload" => Self::ProcessUpload,
            "processAllUploads" => Self::ProcessAllUploads,
            "publishVideo" => Self::PublishVideo,
            "updateVideo" => Self::UpdateVideo,
            "createPlaylist" => Self::CreatePlaylist,
            "addToPlaylist" => Self::AddToPlaylist,
            "ingestGmail" => Self::IngestGmail,
            "sendDigest" => Self::SendDigest,
            "partnerStatus" => Self::PartnerStatus,
            "partnerOwners" => Self::PartnerOwners,
            "partnerClaims" => Self::PartnerClaims,
            "adminStatus" => Self::AdminStatus,
            "workspaceUsers" => Self::WorkspaceUsers,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Channel => "channel",
            Self::Videos => "videos",
            Self::Analytics => "analytics",
            Self::ExportAnalytics => "exportAnalytics",
            Self::Jobs => "jobs",
            Self::StartUpload => "startUpload",
            Self::ProcessUpload => "processUpload",
            Self::ProcessAllUploads => "processAllUploads",
            Self::PublishVideo => "publishVideo",
            Self::UpdateVideo => "updateVideo",
            Self::CreatePlaylist => "createPlaylist",
            Self::AddToPlaylist => "addToPlaylist",
            Self::IngestGmail => "ingestGmail",
            Self::SendDigest => "sendDigest",
            Self::PartnerStatus => "partnerStatus",
            Self::PartnerOwners => "partnerOwners",
            Self::PartnerClaims => "partnerClaims",
            Self::AdminStatus => "adminStatus",
            Self::WorkspaceUsers => "workspaceUsers",
        }
    }

    pub fn is_mutating(self) -> bool {
        matches!(
            self,
            Self::ExportAnalytics
                | Self::StartUpload
                | Self::ProcessUpload
                | Self::ProcessAllUploads
                | Self::PublishVideo
                | Self::UpdateVideo
                | Self::CreatePlaylist
                | Self::AddToPlaylist
                | Self::IngestGmail
                | Self::SendDigest
        )
    }
}

fn prepare_payload(
    action: YoutubeAction,
    payload: Value,
    idempotency_key: Option<&str>,
    allow_public_actions: bool,
) -> Result<Map<String, Value>, YoutubeClientError> {
    let mut object = match payload {
        Value::Null => Map::new(),
        Value::Object(object) => object,
        _ => {
            return Err(YoutubeClientError::Validation(
                "request body must be a JSON object".to_string(),
            ));
        }
    };
    object.remove("action");
    object.remove("apiKey");

    if action.is_mutating() {
        let idempotency_key = idempotency_key.ok_or_else(|| {
            YoutubeClientError::Validation(
                "mutating actions require an Idempotency-Key header".to_string(),
            )
        })?;
        validate_idempotency_key(idempotency_key)?;
        object.insert(
            "controlRequestId".to_string(),
            Value::String(idempotency_key.to_string()),
        );
    }

    match action {
        YoutubeAction::StartUpload => {
            require_non_empty_string(&object, "driveFileId")?;
            require_non_empty_string(&object, "title")?;
            if object.get("rightsConfirmed") != Some(&Value::Bool(true)) {
                return Err(YoutubeClientError::Validation(
                    "startUpload requires rightsConfirmed=true".to_string(),
                ));
            }
            if let Some(status) = optional_string(&object, "privacyStatus")? {
                if status != "private" {
                    return Err(YoutubeClientError::Validation(
                        "startUpload must remain private; publish in a separate approved action"
                            .to_string(),
                    ));
                }
            }

            let header_key = idempotency_key.expect("mutating actions already require a key");
            if let Some(existing) = optional_string(&object, "idempotencyKey")? {
                if existing != header_key {
                    return Err(YoutubeClientError::Validation(
                        "body idempotencyKey must match the Idempotency-Key header".to_string(),
                    ));
                }
            }
            object.insert(
                "idempotencyKey".to_string(),
                Value::String(header_key.to_string()),
            );
        }
        YoutubeAction::PublishVideo => {
            let video_id = require_non_empty_string(&object, "videoId")?;
            let privacy = require_non_empty_string(&object, "privacyStatus")?;
            if !matches!(privacy, "private" | "unlisted" | "public") {
                return Err(YoutubeClientError::Validation(
                    "privacyStatus must be private, unlisted, or public".to_string(),
                ));
            }
            if privacy != "private" && !allow_public_actions {
                return Err(YoutubeClientError::Validation(
                    "public and unlisted actions are disabled by YOUTUBE_ALLOW_PUBLIC_ACTIONS"
                        .to_string(),
                ));
            }
            let expected = format!("PUBLISH {video_id} AS {}", privacy.to_ascii_uppercase());
            let confirmation = require_non_empty_string(&object, "confirmation")?;
            if confirmation != expected {
                return Err(YoutubeClientError::Validation(format!(
                    "confirmation must exactly equal {expected:?}"
                )));
            }
        }
        YoutubeAction::CreatePlaylist => {
            require_non_empty_string(&object, "title")?;
            let privacy = optional_string(&object, "privacyStatus")?.unwrap_or("private");
            if privacy != "private" && !allow_public_actions {
                return Err(YoutubeClientError::Validation(
                    "non-private playlists are disabled by YOUTUBE_ALLOW_PUBLIC_ACTIONS"
                        .to_string(),
                ));
            }
        }
        _ => {}
    }

    Ok(object)
}

fn require_non_empty_string<'a>(
    object: &'a Map<String, Value>,
    field: &str,
) -> Result<&'a str, YoutubeClientError> {
    optional_string(object, field)?.ok_or_else(|| {
        YoutubeClientError::Validation(format!("{field} is required and must be non-empty"))
    })
}

fn optional_string<'a>(
    object: &'a Map<String, Value>,
    field: &str,
) -> Result<Option<&'a str>, YoutubeClientError> {
    match object.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(Some(value.trim())),
        Some(Value::String(_)) => Err(YoutubeClientError::Validation(format!(
            "{field} must not be empty"
        ))),
        Some(_) => Err(YoutubeClientError::Validation(format!(
            "{field} must be a string"
        ))),
    }
}

fn validate_idempotency_key(value: &str) -> Result<(), YoutubeClientError> {
    if value.is_empty() || value.len() > 200 {
        return Err(YoutubeClientError::Validation(
            "Idempotency-Key must contain 1 to 200 characters".to_string(),
        ));
    }
    if !value.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(YoutubeClientError::Validation(
            "Idempotency-Key must contain printable ASCII without spaces".to_string(),
        ));
    }
    Ok(())
}

#[derive(Debug, Deserialize, Serialize)]
struct GasEnvelope {
    ok: bool,
    #[serde(default)]
    data: Option<Value>,
    #[serde(default)]
    error: Option<GasError>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GasError {
    pub message: String,
    pub code: String,
    #[serde(default)]
    pub details: Option<Value>,
}

#[derive(Debug)]
pub enum YoutubeClientError {
    Http(reqwest::Error),
    ResponseTooLarge {
        max_bytes: usize,
    },
    UpstreamHttp {
        status: StatusCode,
        final_url: Url,
        location: Option<String>,
        body: String,
    },
    InvalidJson {
        source: serde_json::Error,
        preview: String,
    },
    AppsScript(GasError),
    Validation(String),
}

impl YoutubeClientError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Http(_) => "YOUTUBE_GAS_HTTP_ERROR",
            Self::ResponseTooLarge { .. } => "YOUTUBE_GAS_RESPONSE_TOO_LARGE",
            Self::UpstreamHttp {
                status, location, ..
            } if status.is_redirection()
                && location
                    .as_deref()
                    .is_some_and(|value| value.contains("accounts.google.com")) =>
            {
                "YOUTUBE_GAS_OWNER_ONLY"
            }
            Self::UpstreamHttp { .. } => "YOUTUBE_GAS_UPSTREAM_HTTP_ERROR",
            Self::InvalidJson { .. } => "YOUTUBE_GAS_INVALID_JSON",
            Self::AppsScript(_) => "YOUTUBE_GAS_APPLICATION_ERROR",
            Self::Validation(_) => "VALIDATION_ERROR",
        }
    }

    pub fn details(&self) -> Option<Value> {
        match self {
            Self::AppsScript(error) => Some(serde_json::json!({
                "upstreamCode": error.code,
                "upstreamDetails": error.details,
            })),
            Self::UpstreamHttp {
                status,
                final_url,
                location,
                body,
            } => Some(serde_json::json!({
                "status": status.as_u16(),
                "finalHost": final_url.host_str(),
                "locationHost": location
                    .as_deref()
                    .and_then(|value| Url::parse(value).ok())
                    .and_then(|url| url.host_str().map(ToOwned::to_owned)),
                "bodyPreview": body,
            })),
            Self::ResponseTooLarge { max_bytes } => {
                Some(serde_json::json!({ "maxBytes": max_bytes }))
            }
            _ => None,
        }
    }
}

impl fmt::Display for YoutubeClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(error) => write!(formatter, "Apps Script request failed: {error}"),
            Self::ResponseTooLarge { max_bytes } => {
                write!(formatter, "Apps Script response exceeded {max_bytes} bytes")
            }
            Self::UpstreamHttp {
                status,
                final_url,
                location,
                ..
            } if status.is_redirection() => write!(
                formatter,
                "Apps Script returned redirect {status} from {final_url}; target was {location:?}. The deployment may still be owner-only"
            ),
            Self::UpstreamHttp { status, .. } => {
                write!(formatter, "Apps Script returned HTTP {status}")
            }
            Self::InvalidJson { source, preview } => write!(
                formatter,
                "Apps Script returned invalid JSON ({source}); preview: {preview}"
            ),
            Self::AppsScript(error) => {
                write!(formatter, "Apps Script {}: {}", error.code, error.message)
            }
            Self::Validation(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for YoutubeClientError {}

pub fn redact_map_for_audit(payload: &Value) -> HashMap<&'static str, Value> {
    let mut audit = HashMap::new();
    if let Some(object) = payload.as_object() {
        for field in [
            "videoId",
            "driveFileId",
            "playlistId",
            "privacyStatus",
            "repository",
            "commit",
        ] {
            if let Some(value) = object.get(field) {
                audit.insert(field, value.clone());
            }
        }
    }
    audit
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{YoutubeAction, prepare_payload};

    #[test]
    fn parses_only_supported_actions() {
        assert_eq!(
            YoutubeAction::parse("channel"),
            Some(YoutubeAction::Channel)
        );
        assert_eq!(YoutubeAction::parse("rotateApiKey"), None);
        assert_eq!(YoutubeAction::parse("setup"), None);
    }

    #[test]
    fn uploads_are_private_and_idempotent() {
        let payload = prepare_payload(
            YoutubeAction::StartUpload,
            json!({
                "driveFileId": "drive-1",
                "title": "A title",
                "rightsConfirmed": true
            }),
            Some("upload-2026-07-27-001"),
            false,
        )
        .expect("valid upload");
        assert_eq!(payload["idempotencyKey"], "upload-2026-07-27-001");
        assert_eq!(payload["controlRequestId"], "upload-2026-07-27-001");
    }

    #[test]
    fn rejects_public_upload_shortcut() {
        let error = prepare_payload(
            YoutubeAction::StartUpload,
            json!({
                "driveFileId": "drive-1",
                "title": "A title",
                "rightsConfirmed": true,
                "privacyStatus": "public"
            }),
            Some("upload-1"),
            true,
        )
        .expect_err("upload must remain private");
        assert!(error.to_string().contains("must remain private"));
    }

    #[test]
    fn public_publish_requires_both_switch_and_exact_phrase() {
        let payload = json!({
            "videoId": "abc123",
            "privacyStatus": "public",
            "confirmation": "PUBLISH abc123 AS PUBLIC"
        });
        assert!(
            prepare_payload(
                YoutubeAction::PublishVideo,
                payload.clone(),
                Some("publish-1"),
                false
            )
            .is_err()
        );
        assert!(
            prepare_payload(
                YoutubeAction::PublishVideo,
                payload,
                Some("publish-1"),
                true
            )
            .is_ok()
        );
    }

    #[test]
    fn mutating_actions_require_idempotency_header() {
        assert!(prepare_payload(YoutubeAction::SendDigest, json!({}), None, false).is_err());
        assert!(prepare_payload(YoutubeAction::Channel, json!({}), None, false).is_ok());
    }
}
