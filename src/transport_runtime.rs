//! Live bounded adapters for stateful mTLS/TCP and durable NATS JetStream.
//!
//! These transports carry the same strict operation envelope as HTTP.  mTLS
//! authenticates the peer connection, but every frame still carries a user
//! bearer that is verified by Shared Auth and bound to `envelope.subject`.
//! JetStream processing uses a database inbox/status/outbox journal so broker
//! redelivery is idempotent and result publication can resume after a crash.

use std::{io, sync::Arc, time::Duration};

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, Statement,
    TransactionTrait,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::Semaphore,
};
use tokio_rustls::{TlsAcceptor, server::TlsStream};
use tokio_util::codec::{Framed, LengthDelimitedCodec};

use crate::web_data_plane::{OperationEnvelope, WebApiMode};

pub const JETSTREAM_REQUEST_SUBJECT: &str = "act.operations.api.v1";
pub const JETSTREAM_RESULT_SUBJECT: &str = "act.results.api.v1";
pub const JETSTREAM_REQUEST_STREAM: &str = "ACT_API_OPERATIONS";
pub const JETSTREAM_RESULT_STREAM: &str = "ACT_API_OPERATION_RESULTS";
pub const JETSTREAM_DURABLE_CONSUMER: &str = "act-api-server-v1";
pub const MAX_AUTHORIZATION_BYTES: usize = 16 * 1024;
pub const MAX_TRANSPORT_BYTES: usize = 64 * 1024;
pub const MAX_TCP_CONNECTIONS: usize = 128;
pub const MAX_CONCURRENT_OPERATIONS: usize = 32;
pub const OPERATION_TIMEOUT: Duration = Duration::from_secs(10);

const CLAIM_INBOX_SQL: &str = "INSERT INTO act_operation_inbox (operation_id, subject, request_hash, status, received_at) VALUES ($1, $2, $3, 'processing', CURRENT_TIMESTAMP) ON CONFLICT (operation_id) DO NOTHING";
const READ_INBOX_SQL: &str = "SELECT subject, request_hash, status, result_json FROM act_operation_inbox WHERE operation_id = $1";
const UPSERT_PROCESSING_STATUS_SQL: &str = "INSERT INTO act_operation_status (operation_id, subject, status, updated_at) VALUES ($1, $2, 'processing', CURRENT_TIMESTAMP) ON CONFLICT (operation_id) DO NOTHING";
const COMPLETE_INBOX_SQL: &str = "UPDATE act_operation_inbox SET status = 'completed', result_json = $2, completed_at = CURRENT_TIMESTAMP WHERE operation_id = $1 AND status = 'processing'";
const UPSERT_COMPLETED_STATUS_SQL: &str = "INSERT INTO act_operation_status (operation_id, status, result_json, updated_at) VALUES ($1, 'completed', $2, CURRENT_TIMESTAMP) ON CONFLICT (operation_id) DO UPDATE SET status = EXCLUDED.status, result_json = EXCLUDED.result_json, updated_at = EXCLUDED.updated_at";
const INSERT_OUTBOX_SQL: &str = "INSERT INTO act_operation_outbox (event_id, operation_id, subject, payload_json, created_at) VALUES ($1, $1, $2, $3, CURRENT_TIMESTAMP) ON CONFLICT (event_id) DO NOTHING";
const READ_OUTBOX_SQL: &str = "SELECT subject, payload_json FROM act_operation_outbox WHERE event_id = $1 AND delivered_at IS NULL";
const MARK_OUTBOX_SQL: &str = "UPDATE act_operation_outbox SET delivered_at = CURRENT_TIMESTAMP WHERE event_id = $1 AND delivered_at IS NULL";
const READ_STATUS_SQL: &str =
    "SELECT status, result_json FROM act_operation_status WHERE operation_id = $1 AND subject = $2";

/// Schema owned by the migration pipeline. Runtime code never executes it.
pub const DURABLE_SCHEMA_SQL: &str = include_str!("../migrations/0001_durable_operations.sql");

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AuthenticatedOperation {
    pub authorization: String,
    pub envelope: OperationEnvelope,
}

impl AuthenticatedOperation {
    pub fn validate(&self, mode: WebApiMode) -> Result<(), TransportError> {
        validate_authorization(&self.authorization)?;
        self.envelope
            .validate_for(mode)
            .map_err(|_| TransportError::InvalidRequest)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OperationReply {
    pub operation_id: Option<String>,
    pub result: Option<Value>,
    pub error: Option<String>,
}

impl OperationReply {
    fn ok(operation_id: String, result: Value) -> Self {
        Self {
            operation_id: Some(operation_id),
            result: Some(result),
            error: None,
        }
    }

    fn error(operation_id: Option<String>, code: &'static str) -> Self {
        Self {
            operation_id,
            result: None,
            error: Some(code.to_string()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportError {
    InvalidRequest,
    Unauthorized,
    AuthUnavailable,
    UnsupportedOperation,
    Timeout,
    Database,
    InvalidState,
    Transport,
}

impl TransportError {
    fn code(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::Unauthorized => "unauthorized",
            Self::AuthUnavailable => "auth_unavailable",
            Self::UnsupportedOperation => "unsupported_operation",
            Self::Timeout => "operation_timeout",
            Self::Database | Self::InvalidState | Self::Transport => "temporarily_unavailable",
        }
    }
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for TransportError {}

#[async_trait]
pub trait OperationService: Send + Sync {
    /// Authenticate the bearer, bind its verified subject to the envelope, and
    /// apply product authorization before returning a result.
    async fn execute(
        &self,
        request: &AuthenticatedOperation,
        mode: WebApiMode,
    ) -> Result<Value, TransportError>;
}

async fn process_payload(
    service: &dyn OperationService,
    payload: &[u8],
    mode: WebApiMode,
) -> OperationReply {
    if payload.len() > MAX_TRANSPORT_BYTES {
        return OperationReply::error(None, "invalid_request");
    }
    let request: AuthenticatedOperation = match serde_json::from_slice(payload) {
        Ok(request) => request,
        Err(_) => return OperationReply::error(None, "invalid_request"),
    };
    let operation_id = request.envelope.operation_id.clone();
    if let Err(error) = request.validate(mode) {
        return OperationReply::error(Some(operation_id), error.code());
    }
    match tokio::time::timeout(OPERATION_TIMEOUT, service.execute(&request, mode)).await {
        Ok(Ok(result)) => OperationReply::ok(operation_id, result),
        Ok(Err(error)) => OperationReply::error(Some(operation_id), error.code()),
        Err(_) => OperationReply::error(Some(operation_id), TransportError::Timeout.code()),
    }
}

pub async fn serve_mtls_tcp(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    service: Arc<dyn OperationService>,
    max_connections: usize,
) -> io::Result<()> {
    if !(1..=MAX_TCP_CONNECTIONS).contains(&max_connections) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "max_connections is outside the reviewed bound",
        ));
    }
    let permits = Arc::new(Semaphore::new(max_connections));
    loop {
        let (stream, peer) = listener.accept().await?;
        let Ok(permit) = permits.clone().try_acquire_owned() else {
            tracing::warn!(%peer, outcome = "backpressure", "mTLS connection rejected at capacity");
            drop(stream);
            continue;
        };
        let acceptor = acceptor.clone();
        let service = service.clone();
        tokio::spawn(async move {
            let _permit = permit;
            match tokio::time::timeout(OPERATION_TIMEOUT, acceptor.accept(stream)).await {
                Ok(Ok(tls)) => {
                    if serve_tls_connection(tls, service).await.is_err() {
                        tracing::warn!(%peer, outcome = "closed", "mTLS operation connection closed");
                    }
                }
                _ => tracing::warn!(%peer, outcome = "tls_rejected", "mTLS handshake failed"),
            }
        });
    }
}

fn length_codec() -> LengthDelimitedCodec {
    LengthDelimitedCodec::builder()
        .length_field_length(4)
        .max_frame_length(MAX_TRANSPORT_BYTES)
        .new_codec()
}

async fn serve_tls_connection(
    stream: TlsStream<TcpStream>,
    service: Arc<dyn OperationService>,
) -> Result<(), TransportError> {
    let mut framed = Framed::new(stream, length_codec());
    while let Some(frame) = framed.next().await {
        let reply = match frame {
            Ok(frame) => {
                process_payload(service.as_ref(), &frame, WebApiMode::StatefulMtlsTcp).await
            }
            Err(_) => OperationReply::error(None, "invalid_request"),
        };
        let encoded = serde_json::to_vec(&reply).map_err(|_| TransportError::Transport)?;
        if encoded.len() > MAX_TRANSPORT_BYTES {
            return Err(TransportError::Transport);
        }
        tokio::time::timeout(OPERATION_TIMEOUT, framed.send(encoded.into()))
            .await
            .map_err(|_| TransportError::Timeout)?
            .map_err(|_| TransportError::Transport)?;
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JournalClaim {
    New,
    Replay(Vec<u8>),
    Processing,
    Conflict,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OperationStatus {
    pub operation_id: String,
    pub status: String,
    pub result: Option<Value>,
}

#[async_trait]
pub trait OperationJournal: Send + Sync {
    async fn claim(
        &self,
        operation_id: &str,
        subject: &str,
        request_json: &str,
    ) -> Result<JournalClaim, TransportError>;
    async fn complete(&self, operation_id: &str, result_json: &[u8]) -> Result<(), TransportError>;
    async fn pending_result(
        &self,
        operation_id: &str,
    ) -> Result<Option<(String, Vec<u8>)>, TransportError>;
    async fn mark_result_published(&self, operation_id: &str) -> Result<(), TransportError>;
    async fn status(
        &self,
        operation_id: &str,
        subject: &str,
    ) -> Result<Option<OperationStatus>, TransportError>;
}

pub struct SeaOrmOperationJournal {
    database: DatabaseConnection,
}

impl SeaOrmOperationJournal {
    #[must_use]
    pub fn new(database: DatabaseConnection) -> Self {
        Self { database }
    }

    async fn begin(&self) -> Result<DatabaseTransaction, TransportError> {
        self.database
            .begin()
            .await
            .map_err(|_| TransportError::Database)
    }
}

#[async_trait]
impl OperationJournal for SeaOrmOperationJournal {
    async fn claim(
        &self,
        operation_id: &str,
        subject: &str,
        request_json: &str,
    ) -> Result<JournalClaim, TransportError> {
        let request_hash = request_fingerprint(request_json.as_bytes());
        let transaction = self.begin().await?;
        let inserted = transaction
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                CLAIM_INBOX_SQL,
                vec![
                    operation_id.into(),
                    subject.into(),
                    request_hash.clone().into(),
                ],
            ))
            .await
            .map_err(|_| TransportError::Database)?;
        if inserted.rows_affected() == 1 {
            transaction
                .execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    UPSERT_PROCESSING_STATUS_SQL,
                    vec![operation_id.into(), subject.into()],
                ))
                .await
                .map_err(|_| TransportError::Database)?;
            transaction
                .commit()
                .await
                .map_err(|_| TransportError::Database)?;
            return Ok(JournalClaim::New);
        }
        let row = transaction
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                READ_INBOX_SQL,
                vec![operation_id.into()],
            ))
            .await
            .map_err(|_| TransportError::Database)?
            .ok_or(TransportError::InvalidState)?;
        let stored_request_hash: String = row
            .try_get("", "request_hash")
            .map_err(|_| TransportError::InvalidState)?;
        let stored_subject: String = row
            .try_get("", "subject")
            .map_err(|_| TransportError::InvalidState)?;
        let status: String = row
            .try_get("", "status")
            .map_err(|_| TransportError::InvalidState)?;
        let result: Option<String> = row
            .try_get("", "result_json")
            .map_err(|_| TransportError::InvalidState)?;
        transaction
            .commit()
            .await
            .map_err(|_| TransportError::Database)?;
        if stored_request_hash != request_hash || stored_subject != subject {
            return Ok(JournalClaim::Conflict);
        }
        match (status.as_str(), result) {
            ("processing", _) => Ok(JournalClaim::Processing),
            ("completed", Some(result)) => Ok(JournalClaim::Replay(result.into_bytes())),
            _ => Err(TransportError::InvalidState),
        }
    }

    async fn complete(&self, operation_id: &str, result_json: &[u8]) -> Result<(), TransportError> {
        let result = std::str::from_utf8(result_json).map_err(|_| TransportError::InvalidState)?;
        let transaction = self.begin().await?;
        let updated = transaction
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                COMPLETE_INBOX_SQL,
                vec![operation_id.into(), result.into()],
            ))
            .await
            .map_err(|_| TransportError::Database)?;
        if updated.rows_affected() != 1 {
            return Err(TransportError::InvalidState);
        }
        transaction
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                UPSERT_COMPLETED_STATUS_SQL,
                vec![operation_id.into(), result.into()],
            ))
            .await
            .map_err(|_| TransportError::Database)?;
        transaction
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                INSERT_OUTBOX_SQL,
                vec![
                    operation_id.into(),
                    JETSTREAM_RESULT_SUBJECT.into(),
                    result.into(),
                ],
            ))
            .await
            .map_err(|_| TransportError::Database)?;
        transaction
            .commit()
            .await
            .map_err(|_| TransportError::Database)
    }

    async fn pending_result(
        &self,
        operation_id: &str,
    ) -> Result<Option<(String, Vec<u8>)>, TransportError> {
        self.database
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                READ_OUTBOX_SQL,
                vec![operation_id.into()],
            ))
            .await
            .map_err(|_| TransportError::Database)?
            .map(|row| {
                let subject: String = row
                    .try_get("", "subject")
                    .map_err(|_| TransportError::InvalidState)?;
                let payload: String = row
                    .try_get("", "payload_json")
                    .map_err(|_| TransportError::InvalidState)?;
                Ok((subject, payload.into_bytes()))
            })
            .transpose()
    }

    async fn mark_result_published(&self, operation_id: &str) -> Result<(), TransportError> {
        self.database
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                MARK_OUTBOX_SQL,
                vec![operation_id.into()],
            ))
            .await
            .map(|_| ())
            .map_err(|_| TransportError::Database)
    }

    async fn status(
        &self,
        operation_id: &str,
        subject: &str,
    ) -> Result<Option<OperationStatus>, TransportError> {
        self.database
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                READ_STATUS_SQL,
                vec![operation_id.into(), subject.into()],
            ))
            .await
            .map_err(|_| TransportError::Database)?
            .map(|row| {
                let status: String = row
                    .try_get("", "status")
                    .map_err(|_| TransportError::InvalidState)?;
                let result: Option<String> = row
                    .try_get("", "result_json")
                    .map_err(|_| TransportError::InvalidState)?;
                let result = result
                    .map(|value| serde_json::from_str(&value))
                    .transpose()
                    .map_err(|_| TransportError::InvalidState)?;
                Ok(OperationStatus {
                    operation_id: operation_id.to_string(),
                    status,
                    result,
                })
            })
            .transpose()
    }
}

#[derive(Clone, Debug)]
pub struct JetStreamConfig {
    pub request_subject: String,
    pub result_subject: String,
    pub request_stream: String,
    pub result_stream: String,
    pub durable_consumer: String,
    pub max_concurrency: usize,
}

impl Default for JetStreamConfig {
    fn default() -> Self {
        Self {
            request_subject: JETSTREAM_REQUEST_SUBJECT.to_string(),
            result_subject: JETSTREAM_RESULT_SUBJECT.to_string(),
            request_stream: JETSTREAM_REQUEST_STREAM.to_string(),
            result_stream: JETSTREAM_RESULT_STREAM.to_string(),
            durable_consumer: JETSTREAM_DURABLE_CONSUMER.to_string(),
            max_concurrency: MAX_CONCURRENT_OPERATIONS,
        }
    }
}

pub async fn serve_jetstream(
    context: async_nats::jetstream::Context,
    service: Arc<dyn OperationService>,
    journal: Arc<dyn OperationJournal>,
    config: JetStreamConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !(1..=MAX_CONCURRENT_OPERATIONS).contains(&config.max_concurrency) {
        return Err("JetStream max_concurrency is outside the reviewed bound".into());
    }
    context
        .get_or_create_stream(async_nats::jetstream::stream::Config {
            name: config.request_stream.clone(),
            subjects: vec![config.request_subject.clone()],
            retention: async_nats::jetstream::stream::RetentionPolicy::WorkQueue,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
            max_message_size: i32::try_from(MAX_TRANSPORT_BYTES).unwrap_or(i32::MAX),
            duplicate_window: Duration::from_secs(120),
            ..Default::default()
        })
        .await?;
    context
        .get_or_create_stream(async_nats::jetstream::stream::Config {
            name: config.result_stream.clone(),
            subjects: vec![config.result_subject.clone()],
            retention: async_nats::jetstream::stream::RetentionPolicy::Limits,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
            max_message_size: i32::try_from(MAX_TRANSPORT_BYTES).unwrap_or(i32::MAX),
            duplicate_window: Duration::from_secs(120),
            ..Default::default()
        })
        .await?;
    let stream = context.get_stream(&config.request_stream).await?;
    let consumer = stream
        .get_or_create_consumer::<async_nats::jetstream::consumer::pull::Config>(
            &config.durable_consumer,
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some(config.durable_consumer.clone()),
                filter_subject: config.request_subject.clone(),
                ack_policy: async_nats::jetstream::consumer::AckPolicy::Explicit,
                ack_wait: Duration::from_secs(30),
                max_deliver: 8,
                ..Default::default()
            },
        )
        .await?;
    let permits = Arc::new(Semaphore::new(config.max_concurrency));
    let mut messages = consumer.messages().await?;
    while let Some(message) = messages.next().await {
        let message = message?;
        let permit = permits.clone().acquire_owned().await?;
        let context = context.clone();
        let service = service.clone();
        let journal = journal.clone();
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(error) = handle_jetstream_message(context, service, journal, message).await {
                tracing::error!(error = %error, "durable JetStream operation failed");
            }
        });
    }
    Ok(())
}

async fn handle_jetstream_message(
    context: async_nats::jetstream::Context,
    service: Arc<dyn OperationService>,
    journal: Arc<dyn OperationJournal>,
    message: async_nats::jetstream::Message,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if message.payload.len() > MAX_TRANSPORT_BYTES {
        message
            .ack_with(async_nats::jetstream::AckKind::Term)
            .await?;
        return Ok(());
    }
    let request_json = match std::str::from_utf8(&message.payload) {
        Ok(value) => value,
        Err(_) => {
            message
                .ack_with(async_nats::jetstream::AckKind::Term)
                .await?;
            return Ok(());
        }
    };
    let request: AuthenticatedOperation = match serde_json::from_str(request_json) {
        Ok(request) => request,
        Err(_) => {
            message
                .ack_with(async_nats::jetstream::AckKind::Term)
                .await?;
            return Ok(());
        }
    };
    if request.validate(WebApiMode::JetStreamAsync).is_err() {
        message
            .ack_with(async_nats::jetstream::AckKind::Term)
            .await?;
        return Ok(());
    }
    let operation_id = request.envelope.operation_id.clone();
    let result = match journal
        .claim(&operation_id, &request.envelope.subject, request_json)
        .await?
    {
        JournalClaim::New => {
            let reply = process_payload(
                service.as_ref(),
                &message.payload,
                WebApiMode::JetStreamAsync,
            )
            .await;
            let payload = serde_json::to_vec(&reply)?;
            journal.complete(&operation_id, &payload).await?;
            payload
        }
        JournalClaim::Replay(payload) => payload,
        JournalClaim::Processing => {
            message
                .ack_with(async_nats::jetstream::AckKind::Nak(Some(
                    Duration::from_secs(1),
                )))
                .await?;
            return Ok(());
        }
        JournalClaim::Conflict => {
            message
                .ack_with(async_nats::jetstream::AckKind::Term)
                .await?;
            return Ok(());
        }
    };
    let (subject, pending_payload) = journal
        .pending_result(&operation_id)
        .await?
        .unwrap_or_else(|| (JETSTREAM_RESULT_SUBJECT.to_string(), result));
    let mut headers = async_nats::HeaderMap::new();
    let dedupe_id = format!("act-operation-result:{operation_id}");
    headers.insert("Nats-Msg-Id", dedupe_id.as_str());
    let acknowledgement = tokio::time::timeout(
        OPERATION_TIMEOUT,
        context.publish_with_headers(subject, headers, pending_payload.into()),
    )
    .await
    .map_err(|_| TransportError::Timeout)??;
    tokio::time::timeout(OPERATION_TIMEOUT, acknowledgement)
        .await
        .map_err(|_| TransportError::Timeout)??;
    journal.mark_result_published(&operation_id).await?;
    message.ack().await?;
    Ok(())
}

fn validate_authorization(value: &str) -> Result<(), TransportError> {
    let token = value
        .strip_prefix("Bearer ")
        .ok_or(TransportError::Unauthorized)?;
    if value.len() > MAX_AUTHORIZATION_BYTES
        || token.is_empty()
        || token.trim() != token
        || token.chars().any(char::is_whitespace)
    {
        return Err(TransportError::Unauthorized);
    }
    Ok(())
}

fn request_fingerprint(payload: &[u8]) -> String {
    let digest = Sha256::digest(payload);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        encoded.push(HEX[usize::from(byte >> 4)] as char);
        encoded.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web_data_plane::{DataOperation, MAX_OPERATION_DEADLINE_MS};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct FakeService;

    #[async_trait]
    impl OperationService for FakeService {
        async fn execute(
            &self,
            request: &AuthenticatedOperation,
            _mode: WebApiMode,
        ) -> Result<Value, TransportError> {
            if request.authorization == "Bearer valid" && request.envelope.subject == "actor-1" {
                Ok(serde_json::json!({"configured": true}))
            } else {
                Err(TransportError::Unauthorized)
            }
        }
    }

    fn request(authorization: &str) -> AuthenticatedOperation {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_millis() as u64;
        AuthenticatedOperation {
            authorization: authorization.to_string(),
            envelope: OperationEnvelope {
                version: 1,
                operation_id: "operation-1".to_string(),
                subject: "actor-1".to_string(),
                resource: "youtube_status".to_string(),
                operation: DataOperation::Read,
                payload: serde_json::json!({}),
                deadline_unix_ms: now + MAX_OPERATION_DEADLINE_MS,
                dedupe_key: Some("operation-1".to_string()),
            },
        }
    }

    #[tokio::test]
    async fn every_tcp_and_jetstream_operation_is_authenticated() {
        for mode in [WebApiMode::StatefulMtlsTcp, WebApiMode::JetStreamAsync] {
            let valid = serde_json::to_vec(&request("Bearer valid")).expect("request");
            let invalid = serde_json::to_vec(&request("Bearer invalid")).expect("request");
            assert!(
                process_payload(&FakeService, &valid, mode)
                    .await
                    .error
                    .is_none()
            );
            assert_eq!(
                process_payload(&FakeService, &invalid, mode)
                    .await
                    .error
                    .as_deref(),
                Some("unauthorized")
            );
        }
    }

    #[test]
    fn request_is_strict_bounded_and_rejects_ambiguous_bearers() {
        assert_eq!(
            request("Bearer token extra").validate(WebApiMode::StatefulMtlsTcp),
            Err(TransportError::Unauthorized)
        );
        let encoded = serde_json::to_vec(&request("Bearer valid")).expect("request");
        assert!(encoded.len() <= MAX_TRANSPORT_BYTES);
        let with_extra = String::from_utf8(encoded).expect("utf8").replace(
            "{\"authorization\"",
            "{\"unexpected\":true,\"authorization\"",
        );
        assert!(serde_json::from_str::<AuthenticatedOperation>(&with_extra).is_err());
    }

    #[test]
    fn durable_contract_requires_inbox_status_outbox_and_dedupe_keys() {
        for required in [
            "act_operation_inbox",
            "act_operation_status",
            "act_operation_outbox",
            "operation_id TEXT PRIMARY KEY",
            "event_id TEXT PRIMARY KEY",
            "delivered_at",
        ] {
            assert!(DURABLE_SCHEMA_SQL.contains(required), "missing {required}");
        }
        assert!(CLAIM_INBOX_SQL.contains("ON CONFLICT (operation_id) DO NOTHING"));
        assert!(INSERT_OUTBOX_SQL.contains("ON CONFLICT (event_id) DO NOTHING"));
        assert!(!CLAIM_INBOX_SQL.contains("request_json"));
        assert_eq!(request_fingerprint(b"same"), request_fingerprint(b"same"));
        assert_ne!(
            request_fingerprint(b"same"),
            request_fingerprint(b"different")
        );
    }
}
