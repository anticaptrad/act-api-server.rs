//! Bounded contracts for the four supported web-to-API data paths.
//!
//! Direct database access is a read-only exception. HTTP is the ordinary
//! request/response path. Stateful TCP is length-framed and requires mTLS.
//! NATS/JetStream work is durable, deduplicated, and acknowledged explicitly.

use std::time::{SystemTime, UNIX_EPOCH};

use sea_orm::{DatabaseBackend, Statement, Value};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

pub const MAX_OPERATION_BYTES: usize = 16 * 1024;
pub const MAX_FRAME_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WebApiMode {
    DirectReadOnlyDatabase,
    StatelessHttp,
    StatefulMtlsTcp,
    JetStreamAsync,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DataOperation {
    Read,
    Write,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OperationEnvelope {
    pub version: u8,
    pub operation_id: String,
    pub subject: String,
    pub resource: String,
    pub operation: DataOperation,
    pub payload: JsonValue,
    pub deadline_unix_ms: u64,
    pub dedupe_key: Option<String>,
}

impl OperationEnvelope {
    pub fn validate_for(&self, mode: WebApiMode) -> Result<(), DataPlaneError> {
        if self.version != 1 {
            return Err(DataPlaneError::UnsupportedVersion);
        }
        validate_identifier(&self.operation_id, 128)?;
        validate_identifier(&self.subject, 256)?;
        validate_identifier(&self.resource, 96)?;
        let encoded = serde_json::to_vec(self).map_err(|_| DataPlaneError::Serialization)?;
        if encoded.len() > MAX_OPERATION_BYTES {
            return Err(DataPlaneError::PayloadTooLarge);
        }
        if self.deadline_unix_ms <= unix_time_ms() {
            return Err(DataPlaneError::DeadlineExpired);
        }
        if mode == WebApiMode::DirectReadOnlyDatabase && self.operation != DataOperation::Read {
            return Err(DataPlaneError::DirectDatabaseWrite);
        }
        if mode == WebApiMode::JetStreamAsync {
            validate_identifier(
                self.dedupe_key
                    .as_deref()
                    .ok_or(DataPlaneError::MissingDedupeKey)?,
                128,
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DirectDatabasePolicy {
    pub role_name: String,
    pub read_only_transaction: bool,
    pub statement_timeout_ms: u64,
    pub row_limit: u32,
}

impl DirectDatabasePolicy {
    pub fn validate(&self) -> Result<(), DataPlaneError> {
        if !self.role_name.ends_with("_web_ro") || !self.read_only_transaction {
            return Err(DataPlaneError::UnsafeDirectDatabasePolicy);
        }
        if !(1..=5_000).contains(&self.statement_timeout_ms)
            || !(1..=1_000).contains(&self.row_limit)
        {
            return Err(DataPlaneError::UnsafeDirectDatabasePolicy);
        }
        Ok(())
    }

    /// Builds the only direct database projection exposed to the web tier.
    /// The query is parameterized and actor-scoped; the runtime must also begin
    /// a read-only transaction using the separately provisioned `_web_ro` role.
    pub fn youtube_channel_select(&self, actor: &str) -> Result<Statement, DataPlaneError> {
        self.validate()?;
        validate_identifier(actor, 256)?;
        Ok(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT channel_id, handle, title FROM youtube_channels WHERE owner_subject = $1 ORDER BY channel_id LIMIT $2",
            [
                Value::String(Some(Box::new(actor.to_string()))),
                Value::BigInt(Some(i64::from(self.row_limit))),
            ],
        ))
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StatelessHttpPolicy {
    pub base_url: String,
    pub service_credential_ref: String,
    pub connect_timeout_ms: u64,
    pub request_timeout_ms: u64,
    pub max_response_bytes: usize,
    pub redirects_enabled: bool,
}

impl StatelessHttpPolicy {
    pub fn validate(&self) -> Result<(), DataPlaneError> {
        let url =
            reqwest::Url::parse(&self.base_url).map_err(|_| DataPlaneError::UnsafeHttpPolicy)?;
        let cluster_http = url.scheme() == "http"
            && url
                .host_str()
                .is_some_and(|host| host.ends_with(".svc.cluster.local"));
        if (url.scheme() != "https" && !cluster_http)
            || url.username() != ""
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || self.service_credential_ref.trim().is_empty()
            || self.redirects_enabled
            || !(1..=2_000).contains(&self.connect_timeout_ms)
            || !(1..=10_000).contains(&self.request_timeout_ms)
            || !(1..=1024 * 1024).contains(&self.max_response_bytes)
        {
            return Err(DataPlaneError::UnsafeHttpPolicy);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StatefulMtlsTcpPolicy {
    pub address: String,
    pub server_name: String,
    pub ca_bundle_ref: String,
    pub client_certificate_ref: String,
    pub client_private_key_ref: String,
    pub mutual_tls_required: bool,
    pub connect_timeout_ms: u64,
    pub operation_timeout_ms: u64,
    pub max_frame_bytes: usize,
}

impl StatefulMtlsTcpPolicy {
    pub fn validate(&self) -> Result<(), DataPlaneError> {
        let secret_refs = [
            self.ca_bundle_ref.as_str(),
            self.client_certificate_ref.as_str(),
            self.client_private_key_ref.as_str(),
        ];
        if self.address.parse::<std::net::SocketAddr>().is_err() && !valid_host_port(&self.address)
            || self.server_name.trim().is_empty()
            || secret_refs.iter().any(|value| value.trim().is_empty())
            || !self.mutual_tls_required
            || !(1..=2_000).contains(&self.connect_timeout_ms)
            || !(1..=30_000).contains(&self.operation_timeout_ms)
            || !(1..=MAX_FRAME_BYTES).contains(&self.max_frame_bytes)
        {
            return Err(DataPlaneError::UnsafeTcpPolicy);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JetStreamPolicy {
    pub request_subject: String,
    pub result_subject_prefix: String,
    pub durable_consumer: String,
    pub max_deliveries: u32,
    pub ack_wait_ms: u64,
    pub publish_timeout_ms: u64,
    pub explicit_ack: bool,
}

impl JetStreamPolicy {
    pub fn validate(&self) -> Result<(), DataPlaneError> {
        if !self.request_subject.starts_with("act.operations.")
            || !self.result_subject_prefix.starts_with("act.results.")
            || self.durable_consumer.trim().is_empty()
            || !(1..=20).contains(&self.max_deliveries)
            || !(1_000..=120_000).contains(&self.ack_wait_ms)
            || !(1..=10_000).contains(&self.publish_timeout_ms)
            || !self.explicit_ack
        {
            return Err(DataPlaneError::UnsafeJetStreamPolicy);
        }
        Ok(())
    }
}

pub fn encode_frame(
    envelope: &OperationEnvelope,
    maximum: usize,
) -> Result<Vec<u8>, DataPlaneError> {
    let payload = serde_json::to_vec(envelope).map_err(|_| DataPlaneError::Serialization)?;
    if payload.len() > maximum || maximum > MAX_FRAME_BYTES {
        return Err(DataPlaneError::PayloadTooLarge);
    }
    let length = u32::try_from(payload.len()).map_err(|_| DataPlaneError::PayloadTooLarge)?;
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

pub fn decode_frame(frame: &[u8], maximum: usize) -> Result<OperationEnvelope, DataPlaneError> {
    let prefix: [u8; 4] = frame
        .get(..4)
        .ok_or(DataPlaneError::InvalidFrame)?
        .try_into()
        .map_err(|_| DataPlaneError::InvalidFrame)?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length > maximum || maximum > MAX_FRAME_BYTES || frame.len() != length + 4 {
        return Err(DataPlaneError::InvalidFrame);
    }
    serde_json::from_slice(&frame[4..]).map_err(|_| DataPlaneError::InvalidFrame)
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

fn validate_identifier(value: &str, maximum: usize) -> Result<(), DataPlaneError> {
    if value.is_empty()
        || value.len() > maximum
        || value.chars().any(char::is_control)
        || value.trim() != value
    {
        return Err(DataPlaneError::InvalidIdentifier);
    }
    Ok(())
}

fn valid_host_port(value: &str) -> bool {
    value
        .rsplit_once(':')
        .is_some_and(|(host, port)| !host.is_empty() && port.parse::<u16>().is_ok())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataPlaneError {
    UnsupportedVersion,
    InvalidIdentifier,
    PayloadTooLarge,
    DeadlineExpired,
    DirectDatabaseWrite,
    MissingDedupeKey,
    Serialization,
    UnsafeDirectDatabasePolicy,
    UnsafeHttpPolicy,
    UnsafeTcpPolicy,
    UnsafeJetStreamPolicy,
    InvalidFrame,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(operation: DataOperation) -> OperationEnvelope {
        OperationEnvelope {
            version: 1,
            operation_id: "op-018f".to_string(),
            subject: "shared-user-1".to_string(),
            resource: "youtube_channels".to_string(),
            operation,
            payload: serde_json::json!({"limit": 10}),
            deadline_unix_ms: unix_time_ms() + 30_000,
            dedupe_key: Some("op-018f".to_string()),
        }
    }

    #[test]
    fn direct_database_is_read_only_and_actor_scoped() {
        assert_eq!(
            request(DataOperation::Write).validate_for(WebApiMode::DirectReadOnlyDatabase),
            Err(DataPlaneError::DirectDatabaseWrite)
        );
        let statement = DirectDatabasePolicy {
            role_name: "act_web_ro".to_string(),
            read_only_transaction: true,
            statement_timeout_ms: 2_000,
            row_limit: 100,
        }
        .youtube_channel_select("shared-user-1")
        .unwrap();
        assert!(statement.sql.starts_with("SELECT "));
        assert!(statement.sql.contains("owner_subject = $1"));
    }

    #[test]
    fn stateless_http_rejects_redirects_and_public_cleartext() {
        let mut policy = StatelessHttpPolicy {
            base_url: "https://act-api.internal".to_string(),
            service_credential_ref: "secret/act-web-api".to_string(),
            connect_timeout_ms: 500,
            request_timeout_ms: 4_000,
            max_response_bytes: 64 * 1024,
            redirects_enabled: false,
        };
        assert_eq!(policy.validate(), Ok(()));
        policy.base_url = "http://public.example.test".to_string();
        assert_eq!(policy.validate(), Err(DataPlaneError::UnsafeHttpPolicy));
    }

    #[test]
    fn stateful_tcp_requires_mtls_and_bounded_frames() {
        let mut policy = StatefulMtlsTcpPolicy {
            address: "act-api.internal:7443".to_string(),
            server_name: "act-api.internal".to_string(),
            ca_bundle_ref: "secret/api-ca".to_string(),
            client_certificate_ref: "secret/web-cert".to_string(),
            client_private_key_ref: "secret/web-key".to_string(),
            mutual_tls_required: true,
            connect_timeout_ms: 500,
            operation_timeout_ms: 5_000,
            max_frame_bytes: MAX_FRAME_BYTES,
        };
        assert_eq!(policy.validate(), Ok(()));
        policy.mutual_tls_required = false;
        assert_eq!(policy.validate(), Err(DataPlaneError::UnsafeTcpPolicy));
    }

    #[test]
    fn framed_round_trip_is_exact_and_rejects_trailing_bytes() {
        let envelope = request(DataOperation::Read);
        let frame = encode_frame(&envelope, MAX_FRAME_BYTES).unwrap();
        assert_eq!(decode_frame(&frame, MAX_FRAME_BYTES).unwrap(), envelope);
        let mut invalid = frame;
        invalid.push(0);
        assert_eq!(
            decode_frame(&invalid, MAX_FRAME_BYTES),
            Err(DataPlaneError::InvalidFrame)
        );
    }

    #[test]
    fn jetstream_requires_dedupe_and_explicit_ack() {
        let mut envelope = request(DataOperation::Write);
        envelope.dedupe_key = None;
        assert_eq!(
            envelope.validate_for(WebApiMode::JetStreamAsync),
            Err(DataPlaneError::MissingDedupeKey)
        );
        let policy = JetStreamPolicy {
            request_subject: "act.operations.youtube".to_string(),
            result_subject_prefix: "act.results.youtube".to_string(),
            durable_consumer: "act-api-youtube-v1".to_string(),
            max_deliveries: 5,
            ack_wait_ms: 30_000,
            publish_timeout_ms: 2_000,
            explicit_ack: true,
        };
        assert_eq!(policy.validate(), Ok(()));
    }
}
