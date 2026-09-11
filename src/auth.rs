//! Fail-closed Shared Auth verification for protected API routes.
//!
//! The caller's bearer is introspected through the official client. The
//! independent service credential is attached only to that server-to-server
//! request and is never forwarded, returned, or logged.

use std::sync::Arc;

use axum::http::{HeaderMap, header::AUTHORIZATION};
use shared_auth_client::{ClientError, SharedAuthClient};

const MAX_INTROSPECTION_RESPONSE_BYTES: usize = 64 * 1024;

#[derive(Clone)]
pub struct SharedAuthVerifier {
    client: SharedAuthClient,
    audience: Arc<str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthSubject {
    pub subject: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthFailure {
    Missing,
    Invalid,
    Unavailable,
}

impl SharedAuthVerifier {
    pub fn new(
        base_url: impl Into<String>,
        service_credential: impl Into<String>,
        audience: impl Into<String>,
    ) -> Result<Self, ClientError> {
        let client = SharedAuthClient::try_new(base_url)?
            .with_service_credential(service_credential)
            .with_max_response_bytes(MAX_INTROSPECTION_RESPONSE_BYTES);
        Ok(Self {
            client,
            audience: Arc::from(audience.into()),
        })
    }

    pub async fn verify(
        &self,
        headers: &HeaderMap,
        required_scopes: &[&str],
    ) -> Result<AuthSubject, AuthFailure> {
        let token = bearer_token(headers)?;
        let claims = self
            .client
            .introspect_with_requirements(token, &self.audience, required_scopes)
            .await
            .map_err(map_client_error)?;

        if !claims.active || claims.aud.as_deref() != Some(self.audience.as_ref()) {
            return Err(AuthFailure::Invalid);
        }
        let subject = claims
            .sub
            .filter(|value| !value.trim().is_empty())
            .ok_or(AuthFailure::Invalid)?;
        Ok(AuthSubject { subject })
    }
}

fn bearer_token(headers: &HeaderMap) -> Result<&str, AuthFailure> {
    let raw = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or(AuthFailure::Missing)?;
    let (scheme, token) = raw.split_once(' ').ok_or(AuthFailure::Invalid)?;
    if !scheme.eq_ignore_ascii_case("Bearer")
        || token.is_empty()
        || token.trim() != token
        || token.chars().any(char::is_whitespace)
    {
        return Err(AuthFailure::Invalid);
    }
    Ok(token)
}

fn map_client_error(error: ClientError) -> AuthFailure {
    match error {
        ClientError::Unauthorized | ClientError::InvalidInput(_) => AuthFailure::Invalid,
        ClientError::MissingServiceCredential
        | ClientError::InvalidBaseUrl
        | ClientError::RequestTooLarge { .. }
        | ClientError::ResponseTooLarge { .. }
        | ClientError::Encode { .. }
        | ClientError::Decode { .. }
        | ClientError::Transport(_)
        | ClientError::Status(_)
        | ClientError::InsecureTransport(_) => AuthFailure::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, header::AUTHORIZATION};

    use super::{AuthFailure, SharedAuthVerifier, bearer_token};

    #[test]
    fn bearer_parser_accepts_one_opaque_token() {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer opaque-product-token"),
        );
        assert_eq!(bearer_token(&headers), Ok("opaque-product-token"));
    }

    #[test]
    fn bearer_parser_rejects_missing_malformed_or_ambiguous_values() {
        assert_eq!(bearer_token(&HeaderMap::new()), Err(AuthFailure::Missing));
        for value in [
            "Basic token",
            "Bearer",
            "Bearer token extra",
            "Bearer  token",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(AUTHORIZATION, HeaderValue::from_str(value).unwrap());
            assert_eq!(bearer_token(&headers), Err(AuthFailure::Invalid));
        }
    }

    #[test]
    fn verifier_rejects_public_cleartext_auth_hosts_at_startup() {
        assert!(
            SharedAuthVerifier::new(
                "http://auth.example.test",
                "independent-service-credential",
                "act-api"
            )
            .is_err()
        );
    }
}
