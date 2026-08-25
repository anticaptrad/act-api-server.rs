use std::{
    net::{SocketAddr, TcpListener},
    sync::Arc,
};

use act_api_server::{
    transport_runtime::{
        AuthenticatedOperation, JetStreamConfig, MAX_TCP_CONNECTIONS, OperationJournal,
        OperationService, SeaOrmOperationJournal, TransportError, serve_jetstream, serve_mtls_tcp,
    },
    web_data_plane::{DataOperation, WebApiMode},
};
use async_trait::async_trait;
use axum::http::{HeaderMap, HeaderValue, header::AUTHORIZATION};
use rustls::{
    RootCertStore, ServerConfig,
    pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
    server::WebPkiClientVerifier,
};
use sea_orm::Database;
use serde_json::{Value, json};
use tokio_rustls::TlsAcceptor;

use crate::{auth, config, nats, routes, telemetry, youtube};

mod shutdown;

/// Initialize configuration, observability, fail-soft dependencies, and HTTP.
pub(crate) async fn run() -> anyhow::Result<()> {
    let cfg = config::Config::from_env()?;
    let _telemetry = telemetry::init(&cfg.service_name)?;

    serve(cfg).await?;
    tracing::info!("shutdown complete");
    Ok(())
}

async fn serve(cfg: config::Config) -> anyhow::Result<()> {
    let nats = nats::connect(&cfg.nats_url).await;
    if let Some(client) = nats.clone() {
        nats::spawn_event_subscriber(client);
    }

    let youtube = cfg
        .youtube
        .as_ref()
        .map(youtube::YoutubeGasClient::new)
        .transpose()?;
    let shared_auth = cfg
        .shared_auth
        .as_ref()
        .map(|auth| {
            auth::SharedAuthVerifier::new(
                auth.base_url.clone(),
                auth.service_credential.clone(),
                auth.audience.clone(),
            )
        })
        .transpose()?;

    let operation_service = shared_auth.as_ref().map(|shared_auth| {
        Arc::new(RuntimeOperationService {
            shared_auth: shared_auth.clone(),
            youtube: youtube.clone(),
        }) as Arc<dyn OperationService>
    });

    if let Some(mtls) = cfg.mtls.as_ref() {
        let service = operation_service
            .clone()
            .ok_or_else(|| anyhow::anyhow!("mTLS operations require Shared Auth"))?;
        let acceptor = mtls_acceptor(mtls)?;
        let address: SocketAddr = mtls.address.parse()?;
        let listener = tokio::net::TcpListener::bind(address).await?;
        tokio::spawn(async move {
            if let Err(error) =
                serve_mtls_tcp(listener, acceptor, service, MAX_TCP_CONNECTIONS).await
            {
                tracing::error!(error = %error, "mTLS operation listener stopped");
            }
        });
        tracing::info!(%address, "bounded mTLS operation listener ready");
    }

    let mut operation_journal: Option<Arc<dyn OperationJournal>> = None;
    if let Some(database_url) = cfg.operation_database_url.as_deref() {
        match Database::connect(database_url).await {
            Ok(database) => {
                let journal: Arc<dyn OperationJournal> =
                    Arc::new(SeaOrmOperationJournal::new(database));
                operation_journal = Some(journal.clone());
                if let (Some(client), Some(service)) = (nats.clone(), operation_service) {
                    let context = async_nats::jetstream::new(client);
                    tokio::spawn(async move {
                        if let Err(error) =
                            serve_jetstream(context, service, journal, JetStreamConfig::default())
                                .await
                        {
                            tracing::error!(error = %error, "durable JetStream worker stopped");
                        }
                    });
                    tracing::info!("durable JetStream operation worker starting");
                } else {
                    tracing::warn!("NATS unavailable; durable operation status remains readable");
                }
            }
            Err(error) => {
                tracing::error!(error = %error, "operation journal unavailable; async mode disabled");
            }
        }
    }

    tracing::info!(
        youtube_configured = youtube.is_some(),
        shared_auth_configured = shared_auth.is_some(),
        "control-plane configuration loaded"
    );

    let app = routes::router(routes::AppState {
        nats,
        youtube,
        shared_auth,
        operation_journal,
    });

    let address = bind_address(cfg.port);
    let listener = TcpListener::bind(address)?;
    listener.set_nonblocking(true)?;
    let local_address = listener.local_addr()?;
    tracing::info!(%local_address, service = %cfg.service_name, "act-api-server listening");

    let server_control = axum_server::Handle::new();
    let server = axum_server::from_tcp(listener)
        .handle(server_control.clone())
        .serve(app.into_make_service());
    let server_handle = tokio::spawn(server);

    shutdown::supervise(server_handle, server_control, shutdown::Config::from_env()).await?;
    Ok(())
}

#[derive(Clone)]
struct RuntimeOperationService {
    shared_auth: auth::SharedAuthVerifier,
    youtube: Option<youtube::YoutubeGasClient>,
}

#[async_trait]
impl OperationService for RuntimeOperationService {
    async fn execute(
        &self,
        request: &AuthenticatedOperation,
        _mode: WebApiMode,
    ) -> Result<Value, TransportError> {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&request.authorization)
                .map_err(|_| TransportError::Unauthorized)?,
        );
        let subject = self
            .shared_auth
            .verify(&headers, &["youtube:admin"])
            .await
            .map_err(|failure| match failure {
                auth::AuthFailure::Missing | auth::AuthFailure::Invalid => {
                    TransportError::Unauthorized
                }
                auth::AuthFailure::Unavailable => TransportError::AuthUnavailable,
            })?;
        if subject.subject != request.envelope.subject {
            return Err(TransportError::Unauthorized);
        }
        if request.envelope.resource != "youtube_status"
            || request.envelope.operation != DataOperation::Read
            || request.envelope.payload != json!({})
        {
            return Err(TransportError::UnsupportedOperation);
        }
        let youtube = self
            .youtube
            .as_ref()
            .ok_or(TransportError::UnsupportedOperation)?;
        Ok(json!({
            "configured": true,
            "expectedChannelHandle": youtube.expected_channel_handle(),
            "deploymentId": youtube.deployment_id(),
            "publicActionsEnabled": youtube.allow_public_actions(),
            "appsScriptApiKeyPresent": true,
            "appsScriptApiKeyExposed": false,
        }))
    }
}

fn mtls_acceptor(mtls: &config::MtlsConfig) -> anyhow::Result<TlsAcceptor> {
    let certificates =
        CertificateDer::pem_file_iter(&mtls.certificate_file)?.collect::<Result<Vec<_>, _>>()?;
    if certificates.is_empty() {
        anyhow::bail!("ACT_API_TLS_CERT_FILE contains no certificates");
    }
    let private_key = PrivateKeyDer::from_pem_file(&mtls.private_key_file)
        .map_err(|_| anyhow::anyhow!("ACT_API_TLS_KEY_FILE contains no usable private key"))?;
    let client_certificates =
        CertificateDer::pem_file_iter(&mtls.client_ca_file)?.collect::<Result<Vec<_>, _>>()?;
    if client_certificates.is_empty() {
        anyhow::bail!("ACT_API_CLIENT_CA_FILE contains no certificates");
    }
    let mut roots = RootCertStore::empty();
    for certificate in client_certificates {
        roots.add(certificate)?;
    }
    let verifier = WebPkiClientVerifier::builder(Arc::new(roots)).build()?;
    let tls = ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(certificates, private_key)?;
    Ok(TlsAcceptor::from(Arc::new(tls)))
}

fn bind_address(port: u16) -> SocketAddr {
    SocketAddr::from(([0, 0, 0, 0], port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_address_uses_all_ipv4_interfaces_and_configured_port() {
        assert_eq!(bind_address(8080), "0.0.0.0:8080".parse().unwrap());
        assert_eq!(bind_address(9124), "0.0.0.0:9124".parse().unwrap());
    }
}
