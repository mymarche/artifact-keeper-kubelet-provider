//! The kubelet image credential provider protocol,
//! `credentialprovider.kubelet.k8s.io/v1`.
//!
//! The request carries the pod's ServiceAccount token, so neither type
//! derives `Debug`: nothing here may ever be formatted into a log line.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::Error;

pub const API_VERSION: &str = "credentialprovider.kubelet.k8s.io/v1";
const REQUEST_KIND: &str = "CredentialProviderRequest";
const RESPONSE_KIND: &str = "CredentialProviderResponse";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialProviderRequest {
    pub api_version: String,
    pub kind: String,
    pub image: String,
    #[serde(default)]
    pub service_account_token: Option<String>,
    #[serde(default)]
    pub service_account_annotations: BTreeMap<String, String>,
}

/// A request that passed validation: the image and a non-empty token.
pub struct ValidRequest {
    pub image: String,
    pub token: String,
}

impl CredentialProviderRequest {
    pub fn parse(input: &[u8]) -> Result<Self, Error> {
        // serde_json's message can quote input, and the input holds a token:
        // report only where parsing failed.
        serde_json::from_slice(input).map_err(|e| {
            Error::Usage(format!(
                "stdin is not a valid CredentialProviderRequest ({:?} error at line {}, column {})",
                e.classify(),
                e.line(),
                e.column()
            ))
        })
    }

    pub fn validate(self) -> Result<ValidRequest, Error> {
        if self.api_version != API_VERSION {
            return Err(Error::Usage(format!(
                "unsupported apiVersion {:?}; this plugin supports {API_VERSION}",
                self.api_version
            )));
        }
        if self.kind != REQUEST_KIND {
            return Err(Error::Usage(format!(
                "unsupported kind {:?}; expected {REQUEST_KIND}",
                self.kind
            )));
        }
        match self.service_account_token {
            Some(token) if !token.is_empty() => Ok(ValidRequest {
                image: self.image,
                token,
            }),
            _ => Err(Error::Usage(
                "request has no serviceAccountToken; set tokenAttributes for this provider in \
                 the kubelet CredentialProviderConfig (requires Kubernetes 1.34+)"
                    .into(),
            )),
        }
    }
}

#[derive(Serialize)]
pub struct AuthConfig {
    pub username: String,
    pub password: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialProviderResponse {
    kind: &'static str,
    api_version: &'static str,
    cache_key_type: &'static str,
    cache_duration: String,
    auth: BTreeMap<String, AuthConfig>,
}

impl CredentialProviderResponse {
    /// One credential for `registry`, cached per registry for `cache`.
    /// A zero `cache` tells the kubelet not to cache it at all.
    pub fn for_registry(registry: String, auth: AuthConfig, cache: Duration) -> Self {
        Self {
            kind: RESPONSE_KIND,
            api_version: API_VERSION,
            cache_key_type: "Registry",
            cache_duration: format!("{}s", cache.as_secs()),
            auth: BTreeMap::from([(registry, auth)]),
        }
    }
}

/// How long the kubelet may cache a credential that expires in `expires_in`
/// seconds: the lifetime minus `margin`, and zero (no caching) when the
/// margin eats it all.
pub fn cache_duration(expires_in: u64, margin: Duration) -> Duration {
    Duration::from_secs(
        Duration::from_secs(expires_in)
            .saturating_sub(margin)
            .as_secs(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shape of the request the kubelet sends with tokenAttributes configured,
    // as documented for credentialprovider.kubelet.k8s.io/v1.
    const SAMPLE_REQUEST: &str = r#"{
        "kind": "CredentialProviderRequest",
        "apiVersion": "credentialprovider.kubelet.k8s.io/v1",
        "image": "ak.example.com/payments/api:1.2",
        "serviceAccountToken": "eyJhbGciOiJSUzI1NiJ9.e30.sig",
        "serviceAccountAnnotations": {"example.com/team": "payments"}
    }"#;

    #[test]
    fn parses_documented_request() {
        let req = CredentialProviderRequest::parse(SAMPLE_REQUEST.as_bytes()).unwrap();
        assert_eq!(req.image, "ak.example.com/payments/api:1.2");
        assert_eq!(
            req.service_account_annotations
                .get("example.com/team")
                .map(String::as_str),
            Some("payments")
        );
        let valid = req.validate().unwrap();
        assert_eq!(valid.token, "eyJhbGciOiJSUzI1NiJ9.e30.sig");
    }

    #[test]
    fn serializes_documented_response() {
        let resp = CredentialProviderResponse::for_registry(
            "ak.example.com".into(),
            AuthConfig {
                username: "ci-0123456789ab".into(),
                password: "token".into(),
            },
            Duration::from_secs(840),
        );
        let json: serde_json::Value = serde_json::to_value(&resp).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "kind": "CredentialProviderResponse",
                "apiVersion": "credentialprovider.kubelet.k8s.io/v1",
                "cacheKeyType": "Registry",
                "cacheDuration": "840s",
                "auth": {"ak.example.com": {"username": "ci-0123456789ab", "password": "token"}}
            })
        );
    }

    #[test]
    fn refuses_other_api_version() {
        let input = SAMPLE_REQUEST.replace(
            "credentialprovider.kubelet.k8s.io/v1",
            "credentialprovider.kubelet.k8s.io/v1beta1",
        );
        let err = CredentialProviderRequest::parse(input.as_bytes())
            .unwrap()
            .validate()
            .err()
            .unwrap();
        assert!(err.to_string().contains(API_VERSION), "{err}");
    }

    #[test]
    fn refuses_other_kind() {
        let input = SAMPLE_REQUEST.replace("CredentialProviderRequest", "Something");
        let err = CredentialProviderRequest::parse(input.as_bytes())
            .unwrap()
            .validate()
            .err()
            .unwrap();
        assert!(err.to_string().contains("unsupported kind"), "{err}");
    }

    #[test]
    fn requires_a_service_account_token() {
        for input in [
            r#"{"kind":"CredentialProviderRequest","apiVersion":"credentialprovider.kubelet.k8s.io/v1","image":"ak.example.com/a"}"#,
            r#"{"kind":"CredentialProviderRequest","apiVersion":"credentialprovider.kubelet.k8s.io/v1","image":"ak.example.com/a","serviceAccountToken":""}"#,
        ] {
            let err = CredentialProviderRequest::parse(input.as_bytes())
                .unwrap()
                .validate()
                .err()
                .unwrap();
            assert!(err.to_string().contains("tokenAttributes"), "{err}");
        }
    }

    #[test]
    fn parse_error_does_not_quote_input() {
        let input =
            r#"{"kind":"CredentialProviderRequest","serviceAccountToken":12, "x": "secret-token"#;
        let err = CredentialProviderRequest::parse(input.as_bytes())
            .err()
            .unwrap();
        assert!(!err.to_string().contains("secret-token"), "{err}");
    }

    #[test]
    fn cache_duration_subtracts_margin() {
        assert_eq!(
            cache_duration(900, Duration::from_secs(60)),
            Duration::from_secs(840)
        );
    }

    #[test]
    fn cache_duration_is_zero_when_margin_covers_lifetime() {
        assert_eq!(cache_duration(30, Duration::from_secs(60)), Duration::ZERO);
        assert_eq!(cache_duration(60, Duration::from_secs(60)), Duration::ZERO);
    }

    #[test]
    fn cache_duration_rounds_down_subsecond_margin() {
        assert_eq!(
            cache_duration(900, Duration::from_millis(500)),
            Duration::from_secs(899)
        );
    }
}
