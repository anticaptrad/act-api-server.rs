//! Authentication helpers for the administrative HTTP surface.
//!
//! The Rust API never accepts or returns the Apps Script API key. Callers use a
//! separate `ADMIN_API_KEY`, while the server injects `YOUTUBE_GAS_API_KEY`
//! only into the outbound request body.

use axum::http::{HeaderMap, header::AUTHORIZATION};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthFailure {
    NotConfigured,
    Missing,
    Invalid,
}

pub fn require_bearer(headers: &HeaderMap, expected: Option<&str>) -> Result<(), AuthFailure> {
    let expected = expected.ok_or(AuthFailure::NotConfigured)?;
    let raw = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or(AuthFailure::Missing)?;
    let (scheme, provided) = raw.split_once(' ').ok_or(AuthFailure::Invalid)?;
    if !scheme.eq_ignore_ascii_case("Bearer") || provided.is_empty() {
        return Err(AuthFailure::Invalid);
    }
    if constant_time_equals(provided.as_bytes(), expected.as_bytes()) {
        Ok(())
    } else {
        Err(AuthFailure::Invalid)
    }
}

fn constant_time_equals(left: &[u8], right: &[u8]) -> bool {
    let max_len = left.len().max(right.len());
    let mut difference = left.len() ^ right.len();
    for index in 0..max_len {
        let left_byte = left.get(index).copied().unwrap_or_default();
        let right_byte = right.get(index).copied().unwrap_or_default();
        difference |= usize::from(left_byte ^ right_byte);
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, header::AUTHORIZATION};

    use super::{AuthFailure, constant_time_equals, require_bearer};

    #[test]
    fn validates_bearer_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer this-is-a-long-administrative-api-key"),
        );
        assert_eq!(
            require_bearer(
                &headers,
                Some("this-is-a-long-administrative-api-key")
            ),
            Ok(())
        );
    }

    #[test]
    fn rejects_missing_or_incorrect_credentials() {
        assert_eq!(require_bearer(&HeaderMap::new(), None), Err(AuthFailure::NotConfigured));
        assert_eq!(
            require_bearer(&HeaderMap::new(), Some("expected")),
            Err(AuthFailure::Missing)
        );

        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Basic expected"));
        assert_eq!(
            require_bearer(&headers, Some("expected")),
            Err(AuthFailure::Invalid)
        );
    }

    #[test]
    fn constant_time_comparison_checks_length_and_content() {
        assert!(constant_time_equals(b"same", b"same"));
        assert!(!constant_time_equals(b"same", b"different"));
        assert!(!constant_time_equals(b"prefix", b"prefix-extra"));
    }
}
