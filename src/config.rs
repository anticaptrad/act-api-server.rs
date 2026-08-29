//! Runtime configuration sourced entirely from the environment.
//!
//! Secrets and endpoints are injected by the k8s deployment (see `act-infra`).
//! No `.env` files are used — the `dotenv` crate is blacklisted across all repos.

use std::time::Duration;

use anyhow::{Context, bail};
use reqwest::Url;

const DEFAULT_MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_ALLOWED_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub nats_url: String,
    pub service_name: String,
    pub shared_auth: Option<SharedAuthConfig>,
    pub youtube: Option<YoutubeConfig>,
    pub operation_database_url: Option<String>,
    pub operation_attestation_key: Option<String>,
    pub mtls: Option<MtlsConfig>,
}

#[derive(Debug, Clone)]
pub struct SharedAuthConfig {
    pub base_url: String,
    pub service_credential: String,
    pub audience: String,
}

#[derive(Debug, Clone)]
pub struct MtlsConfig {
    pub address: String,
    pub certificate_file: String,
    pub private_key_file: String,
    pub client_ca_file: String,
}

#[derive(Debug, Clone)]
pub struct YoutubeConfig {
    pub web_app_url: Url,
    pub api_key: String,
    pub expected_channel_handle: String,
    pub timeout: Duration,
    pub max_response_bytes: usize,
    pub allow_public_actions: bool,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let port = parse_env("PORT", 8080_u16)?;
        let nats_url =
            env_non_empty("NATS_URL").unwrap_or_else(|| "nats://localhost:4222".to_string());
        let service_name =
            env_non_empty("OTEL_SERVICE_NAME").unwrap_or_else(|| "act-api-server".to_string());

        let shared_auth_url = env_non_empty("SHARED_AUTH_URL");
        let shared_auth_service_credential = env_non_empty("SHARED_AUTH_SERVICE_CREDENTIAL");
        let shared_auth = match (shared_auth_url, shared_auth_service_credential) {
            (None, None) => None,
            (None, Some(_)) => {
                bail!("SHARED_AUTH_SERVICE_CREDENTIAL is set but SHARED_AUTH_URL is missing")
            }
            (Some(_), None) => {
                bail!("SHARED_AUTH_URL is set but SHARED_AUTH_SERVICE_CREDENTIAL is missing")
            }
            (Some(base_url), Some(service_credential)) => {
                if service_credential.len() < 16 || service_credential.chars().any(char::is_control)
                {
                    bail!("SHARED_AUTH_SERVICE_CREDENTIAL is not a valid service bearer")
                }
                Some(SharedAuthConfig {
                    base_url,
                    service_credential,
                    audience: "act-api".to_string(),
                })
            }
        };

        let gas_url = env_non_empty("YOUTUBE_GAS_URL");
        let gas_api_key = env_non_empty("YOUTUBE_GAS_API_KEY");
        let youtube = match (gas_url, gas_api_key) {
            (None, None) => None,
            (None, Some(_)) => bail!("YOUTUBE_GAS_API_KEY is set but YOUTUBE_GAS_URL is missing"),
            (Some(_), None) => bail!("YOUTUBE_GAS_URL is set but YOUTUBE_GAS_API_KEY is missing"),
            (Some(raw_url), Some(api_key)) => {
                if api_key.len() < 32 {
                    bail!("YOUTUBE_GAS_API_KEY must be at least 32 characters");
                }

                let web_app_url = parse_apps_script_url(&raw_url)?;
                let expected_channel_handle = env_non_empty("YOUTUBE_EXPECTED_CHANNEL_HANDLE")
                    .unwrap_or_else(|| "@anticaptrad".to_string());
                if !expected_channel_handle.starts_with('@') {
                    bail!("YOUTUBE_EXPECTED_CHANNEL_HANDLE must start with @");
                }

                let timeout_secs = parse_env("YOUTUBE_GAS_TIMEOUT_SECS", 30_u64)?;
                if !(1..=120).contains(&timeout_secs) {
                    bail!("YOUTUBE_GAS_TIMEOUT_SECS must be between 1 and 120");
                }

                let max_response_bytes =
                    parse_env("YOUTUBE_GAS_MAX_RESPONSE_BYTES", DEFAULT_MAX_RESPONSE_BYTES)?;
                if !(1024..=MAX_ALLOWED_RESPONSE_BYTES).contains(&max_response_bytes) {
                    bail!(
                        "YOUTUBE_GAS_MAX_RESPONSE_BYTES must be between 1024 and {MAX_ALLOWED_RESPONSE_BYTES}"
                    );
                }

                let allow_public_actions = parse_bool_env("YOUTUBE_ALLOW_PUBLIC_ACTIONS", false)?;

                Some(YoutubeConfig {
                    web_app_url,
                    api_key,
                    expected_channel_handle,
                    timeout: Duration::from_secs(timeout_secs),
                    max_response_bytes,
                    allow_public_actions,
                })
            }
        };

        validate_auth_configuration(shared_auth.is_some(), youtube.is_some())?;
        let operation_database_url = env_non_empty("ACT_OPERATION_DATABASE_URL");
        let operation_attestation_key = env_non_empty("ACT_NATS_OPERATION_HMAC_KEY");
        validate_operation_configuration(
            operation_database_url.is_some(),
            operation_attestation_key.as_deref(),
        )?;
        if operation_database_url.is_some() {
            validate_durable_nats_url(&nats_url)?;
            if shared_auth.is_none() {
                bail!("Shared Auth is required whenever durable JetStream is configured");
            }
        }
        let mtls = parse_mtls_config()?;
        if mtls.is_some() && shared_auth.is_none() {
            bail!("Shared Auth is required whenever the mTLS operation listener is configured");
        }

        Ok(Self {
            port,
            nats_url,
            service_name,
            shared_auth,
            youtube,
            operation_database_url,
            operation_attestation_key,
            mtls,
        })
    }
}

fn parse_mtls_config() -> anyhow::Result<Option<MtlsConfig>> {
    let values = [
        env_non_empty("ACT_API_MTLS_ADDR"),
        env_non_empty("ACT_API_TLS_CERT_FILE"),
        env_non_empty("ACT_API_TLS_KEY_FILE"),
        env_non_empty("ACT_API_CLIENT_CA_FILE"),
    ];
    if values.iter().all(Option::is_none) {
        return Ok(None);
    }
    let [
        Some(address),
        Some(certificate_file),
        Some(private_key_file),
        Some(client_ca_file),
    ] = values
    else {
        bail!("all ACT_API_MTLS_ADDR and ACT_API_* TLS file variables are required together")
    };
    address
        .parse::<std::net::SocketAddr>()
        .context("ACT_API_MTLS_ADDR must be an IP socket address")?;
    Ok(Some(MtlsConfig {
        address,
        certificate_file,
        private_key_file,
        client_ca_file,
    }))
}

fn validate_durable_nats_url(value: &str) -> anyhow::Result<()> {
    let allowed = value.starts_with("tls://")
        || value.starts_with("nats://127.0.0.1")
        || value.starts_with("nats://localhost")
        || value.starts_with("nats://[::1]");
    if !allowed {
        bail!("durable JetStream requires TLS outside explicit loopback development")
    }
    Ok(())
}

fn validate_operation_configuration(
    database_enabled: bool,
    attestation_key: Option<&str>,
) -> anyhow::Result<()> {
    match (database_enabled, attestation_key) {
        (false, None) => Ok(()),
        (true, None) => {
            bail!("ACT_NATS_OPERATION_HMAC_KEY is required for durable JetStream operations")
        }
        (false, Some(_)) => {
            bail!("ACT_OPERATION_DATABASE_URL is required with ACT_NATS_OPERATION_HMAC_KEY")
        }
        (true, Some(key)) => {
            if key.len() < 32 || key.chars().any(char::is_control) {
                bail!("ACT_NATS_OPERATION_HMAC_KEY must contain at least 32 non-control bytes")
            }
            Ok(())
        }
    }
}

fn validate_auth_configuration(
    shared_auth_enabled: bool,
    youtube_enabled: bool,
) -> anyhow::Result<()> {
    if youtube_enabled && !shared_auth_enabled {
        bail!("Shared Auth is required whenever the YouTube GAS control plane is configured");
    }
    Ok(())
}

fn env_non_empty(name: &str) -> Option<String> {
    crate::flags::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_env<T>(name: &str, default: T) -> anyhow::Result<T>
where
    T: std::str::FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    match env_non_empty(name) {
        Some(value) => value
            .parse::<T>()
            .with_context(|| format!("invalid value for {name}")),
        None => Ok(default),
    }
}

fn parse_bool_env(name: &str, default: bool) -> anyhow::Result<bool> {
    match env_non_empty(name) {
        None => Ok(default),
        Some(value) => match value.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            _ => bail!("{name} must be true or false"),
        },
    }
}

fn parse_apps_script_url(raw: &str) -> anyhow::Result<Url> {
    let url = Url::parse(raw).context("YOUTUBE_GAS_URL is not a valid URL")?;
    if url.scheme() != "https" {
        bail!("YOUTUBE_GAS_URL must use https");
    }
    if url.host_str() != Some("script.google.com") {
        bail!("YOUTUBE_GAS_URL must use the script.google.com host");
    }
    if url.query().is_some() || url.fragment().is_some() {
        bail!("YOUTUBE_GAS_URL must not contain a query string or fragment");
    }

    let path = url.path().trim_end_matches('/');
    if !path.starts_with("/macros/s/") || !path.ends_with("/exec") {
        bail!("YOUTUBE_GAS_URL must be a deployed /macros/s/.../exec web-app URL");
    }

    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::{
        parse_apps_script_url, validate_auth_configuration, validate_durable_nats_url,
        validate_operation_configuration,
    };

    #[test]
    fn accepts_deployed_apps_script_url() {
        let url = parse_apps_script_url(
            "https://script.google.com/macros/s/AKfycbwXNUnFogkqg_aeobBMLCas21CHJ8eIR8W1AnmEBNx7pPgfio8eARW5J4A-lu_V5gY/exec",
        )
        .expect("valid deployment URL");
        assert_eq!(url.host_str(), Some("script.google.com"));
    }

    #[test]
    fn rejects_non_google_or_editor_urls() {
        assert!(parse_apps_script_url("https://example.com/macros/s/id/exec").is_err());
        assert!(
            parse_apps_script_url("https://script.google.com/home/projects/example/edit").is_err()
        );
        assert!(
            parse_apps_script_url("https://script.google.com/macros/s/id/exec?apiKey=secret")
                .is_err()
        );
    }

    #[test]
    fn youtube_configuration_requires_shared_auth() {
        assert!(validate_auth_configuration(false, true).is_err());
        assert!(validate_auth_configuration(true, true).is_ok());
        assert!(validate_auth_configuration(false, false).is_ok());
    }

    #[test]
    fn durable_nats_rejects_remote_cleartext() {
        assert!(validate_durable_nats_url("tls://nats.example.test:4222").is_ok());
        assert!(validate_durable_nats_url("nats://127.0.0.1:4222").is_ok());
        assert!(validate_durable_nats_url("nats://nats.example.test:4222").is_err());
    }

    #[test]
    fn durable_operations_require_a_separate_strong_attestation_key() {
        assert!(validate_operation_configuration(false, None).is_ok());
        assert!(validate_operation_configuration(true, None).is_err());
        assert!(validate_operation_configuration(false, Some(&"k".repeat(32))).is_err());
        assert!(validate_operation_configuration(true, Some("too-short")).is_err());
        assert!(validate_operation_configuration(true, Some(&"k".repeat(32))).is_ok());
    }
}
