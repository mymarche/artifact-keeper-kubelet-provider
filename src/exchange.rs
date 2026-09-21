//! The token exchange at `POST {url}/api/v1/auth/ci/token`.
//!
//! The ServiceAccount token is placed in the `Authorization` header and is
//! never formatted into any message. Every text that can reach stderr from
//! here goes through [`sanitize`], which also redacts the token.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use ureq::tls::{Certificate, PemItem, RootCerts, TlsConfig};
use ureq::{Agent, Proxy};

use crate::config::Config;
use crate::error::{Error, sanitize};

const EXCHANGE_PATH: &str = "/api/v1/auth/ci/token";
/// Upper bound on a response body read into memory.
const MAX_BODY: u64 = 64 * 1024;

/// The fields of the exchange response the plugin uses. Not `Debug`: it
/// holds the minted token.
#[derive(Deserialize)]
pub struct ExchangeResponse {
    pub access_token: String,
    pub expires_in: u64,
    pub username: String,
}

#[derive(Deserialize)]
struct ServerError {
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

pub struct Exchanger {
    agent: Agent,
    base_url: String,
    endpoint: String,
    provider_id: Option<String>,
    timeout: Duration,
}

impl Exchanger {
    pub fn new(config: &Config) -> Result<Self, Error> {
        let tls = TlsConfig::builder()
            .root_certs(root_certs(config.ca_file.as_deref())?)
            .build();
        let agent: Agent = Agent::config_builder()
            .timeout_global(Some(config.timeout))
            .https_only(!config.insecure_http)
            .http_status_as_error(false)
            // Never follow a redirect with the token attached.
            .max_redirects(0)
            .max_redirects_will_error(false)
            .proxy(Proxy::try_from_env())
            .tls_config(tls)
            .build()
            .into();
        Ok(Self {
            agent,
            base_url: config.url.clone(),
            endpoint: format!("{}{EXCHANGE_PATH}", config.url),
            provider_id: config.provider_id.clone(),
            timeout: config.timeout,
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn exchange(&self, token: &str) -> Result<ExchangeResponse, Error> {
        let request = self
            .agent
            .post(&self.endpoint)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/json");
        let sent = match &self.provider_id {
            Some(id) => {
                let body = serde_json::json!({ "provider_id": id }).to_string();
                request
                    .header("Content-Type", "application/json")
                    .send(body.as_bytes())
            }
            None => request.send_empty(),
        };
        let mut response = sent.map_err(|e| self.transport_error(e, token))?;

        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_BODY)
            .read_to_vec()
            .map_err(|e| self.transport_error(e, token))?;

        if !(200..300).contains(&status) {
            return Err(self.fail(Some(status), server_message(&body, token)));
        }
        let parsed: ExchangeResponse = serde_json::from_slice(&body).map_err(|e| {
            // The body holds the minted token: report only where it broke.
            self.fail(
                Some(status),
                format!(
                    "malformed exchange response ({:?} error at line {}, column {})",
                    e.classify(),
                    e.line(),
                    e.column()
                ),
            )
        })?;
        if parsed.access_token.is_empty() || parsed.username.is_empty() {
            return Err(self.fail(
                Some(status),
                "exchange response has an empty access_token or username".into(),
            ));
        }
        Ok(parsed)
    }

    fn fail(&self, status: Option<u16>, message: String) -> Error {
        Error::Exchange {
            url: self.base_url.clone(),
            status,
            message,
        }
    }

    fn transport_error(&self, e: ureq::Error, token: &str) -> Error {
        let message = match e {
            ureq::Error::Timeout(_) => {
                format!("timed out after {}", humanize(self.timeout))
            }
            other => sanitize(&other.to_string(), token),
        };
        self.fail(None, message)
    }
}

fn server_message(body: &[u8], token: &str) -> String {
    let text = match serde_json::from_slice::<ServerError>(body) {
        Ok(ServerError {
            message: Some(m), ..
        }) => m,
        Ok(ServerError { error: Some(e), .. }) => e,
        _ => String::from_utf8_lossy(body).into_owned(),
    };
    let text = sanitize(&text, token);
    if text.is_empty() {
        "(no error message)".into()
    } else {
        text
    }
}

fn humanize(d: Duration) -> String {
    if d.subsec_millis() == 0 {
        format!("{}s", d.as_secs())
    } else {
        format!("{}ms", d.as_millis())
    }
}

/// Mozilla roots, plus every certificate in `ca_file` if given.
fn root_certs(ca_file: Option<&Path>) -> Result<RootCerts, Error> {
    let mut certs: Vec<Certificate<'static>> = webpki_root_certs::TLS_SERVER_ROOT_CERTS
        .iter()
        .map(|der| Certificate::from_der(der.as_ref()))
        .collect();
    if let Some(path) = ca_file {
        let pem = std::fs::read(path)
            .map_err(|e| Error::Usage(format!("cannot read --ca-file {}: {e}", path.display())))?;
        let before = certs.len();
        for item in ureq::tls::parse_pem(&pem) {
            match item {
                Ok(PemItem::Certificate(cert)) => certs.push(cert),
                Ok(_) => {}
                Err(e) => {
                    return Err(Error::Usage(format!(
                        "cannot parse --ca-file {}: {e}",
                        path.display()
                    )));
                }
            }
        }
        if certs.len() == before {
            return Err(Error::Usage(format!(
                "--ca-file {} contains no certificates",
                path.display()
            )));
        }
    }
    Ok(RootCerts::Specific(Arc::new(certs)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_message_prefers_message_field() {
        let body = br#"{"error":"AUTH","message":"CI JWT did not match any identity mapping"}"#;
        assert_eq!(
            server_message(body, "tok"),
            "CI JWT did not match any identity mapping"
        );
    }

    #[test]
    fn server_message_falls_back_to_error_then_text() {
        assert_eq!(server_message(br#"{"error":"NOPE"}"#, "tok"), "NOPE");
        assert_eq!(server_message(b"Bad Gateway\n", "tok"), "Bad Gateway");
        assert_eq!(server_message(b"", "tok"), "(no error message)");
    }

    #[test]
    fn server_message_redacts_an_echoed_token() {
        let body = br#"{"message":"invalid token secret.jwt.value"}"#;
        assert_eq!(
            server_message(body, "secret.jwt.value"),
            "invalid token [REDACTED]"
        );
    }

    #[test]
    fn ca_file_without_certificates_is_refused() {
        let dir = std::env::temp_dir().join(format!("akkp-ca-{}", std::process::id()));
        std::fs::write(&dir, b"not a pem").unwrap();
        let err = root_certs(Some(&dir)).err().unwrap();
        std::fs::remove_file(&dir).ok();
        assert!(err.to_string().contains("no certificates"), "{err}");
    }

    #[test]
    fn missing_ca_file_is_refused() {
        let err = root_certs(Some(Path::new("/nonexistent/ca.pem")))
            .err()
            .unwrap();
        assert!(err.to_string().contains("cannot read --ca-file"), "{err}");
    }

    #[test]
    fn humanize_durations() {
        assert_eq!(humanize(Duration::from_secs(10)), "10s");
        assert_eq!(humanize(Duration::from_millis(500)), "500ms");
    }
}
