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
    pub admin_api_key: Option<String>,
    pub youtube: Option<YoutubeConfig>,
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
        let nats_url = env_non_empty("NATS_URL")
            .unwrap_or_else(|| "nats://localhost:4222".to_string());
        let service_name = env_non_empty("OTEL_SERVICE_NAME")
            .unwrap_or_else(|| "act-api-server".to_string());

        let admin_api_key = env_non_empty("ADMIN_API_KEY");
        if let Some(key) = admin_api_key.as_deref() {
            if key.len() < 32 {
                bail!("ADMIN_API_KEY must be at least 32 characters when configured");
            }
        }

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

        Ok(Self {
            port,
            nats_url,
            service_name,
            admin_api_key,
            youtube,
        })
    }
}

fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name)
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
    use super::parse_apps_script_url;

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
}
