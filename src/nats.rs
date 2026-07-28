//! NATS message-queue bridge.
//!
//! Connects to the NATS bridge deployed in the k8s cluster. The connection is
//! treated as optional at boot: if the broker is unreachable the service still
//! starts and serves HTTP, logging a warning, so a transient MQ outage does not
//! take the API down. Reconnection is handled internally by `async-nats`.

use async_nats::Client;
use futures::StreamExt;
use serde_json::Value;

/// Subject subscribed to for platform-wide events.
pub const EVENT_SUBJECT: &str = "act.events.>";

pub async fn connect(url: &str) -> Option<Client> {
    match async_nats::connect(url).await {
        Ok(client) => {
            tracing::info!(%url, "connected to NATS bridge");
            Some(client)
        }
        Err(err) => {
            tracing::warn!(%url, error = %err, "NATS unavailable; continuing without event bus");
            None
        }
    }
}

/// Spawn a background task that logs events published on [`EVENT_SUBJECT`].
pub fn spawn_event_subscriber(client: Client) {
    tokio::spawn(async move {
        match client.subscribe(EVENT_SUBJECT).await {
            Ok(mut subscriber) => {
                tracing::info!(subject = EVENT_SUBJECT, "subscribed to event bus");
                while let Some(message) = subscriber.next().await {
                    tracing::info!(
                        subject = %message.subject,
                        bytes = message.payload.len(),
                        "event received"
                    );
                }
                tracing::warn!("event subscription ended");
            }
            Err(err) => {
                tracing::error!(error = %err, subject = EVENT_SUBJECT, "failed to subscribe");
            }
        }
    });
}

/// Publish a redacted lifecycle event without making NATS a hard dependency.
/// Payloads passed here must never contain API keys, OAuth tokens, resumable
/// upload URLs, Gmail content, descriptions, or other sensitive data.
pub async fn publish_json(client: Option<&Client>, subject: &str, payload: &Value) {
    let Some(client) = client else {
        return;
    };

    let encoded = match serde_json::to_vec(payload) {
        Ok(encoded) => encoded,
        Err(error) => {
            tracing::warn!(%subject, %error, "could not serialize NATS audit event");
            return;
        }
    };

    if let Err(error) = client.publish(subject.to_string(), encoded.into()).await {
        tracing::warn!(%subject, %error, "could not publish NATS audit event");
    }
}
