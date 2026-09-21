//! Kubelet image credential provider for Artifact Keeper.
//!
//! The kubelet (1.34+, with `tokenAttributes` configured) runs this binary
//! with a `CredentialProviderRequest` on stdin that carries the pulling pod's
//! ServiceAccount token. The token is exchanged at Artifact Keeper's CI OIDC
//! endpoint for a short-lived, pull-only credential, which is returned as a
//! `CredentialProviderResponse` on stdout.

#![forbid(unsafe_code)]

pub mod config;
pub mod error;
pub mod exchange;
pub mod image;
pub mod log;
pub mod protocol;

use config::Config;
use error::Error;
use exchange::Exchanger;
use log::Logger;
use protocol::{AuthConfig, CredentialProviderRequest, CredentialProviderResponse, cache_duration};

/// Handle one request: `input` is the kubelet's stdin, the result is the
/// JSON document for stdout.
pub fn run(config: &Config, input: &[u8], log: Logger) -> Result<String, Error> {
    let request = CredentialProviderRequest::parse(input)?.validate()?;
    let registry = image::registry_host(&request.image)?.to_owned();

    let exchanger = Exchanger::new(config)?;
    log.debug(format_args!(
        "exchanging ServiceAccount token for {registry} at {} (provider_id: {})",
        exchanger.endpoint(),
        config.provider_id.as_deref().unwrap_or("none"),
    ));
    let exchanged = exchanger.exchange(&request.token)?;

    let cache = cache_duration(exchanged.expires_in, config.cache_margin);
    log.info(format_args!(
        "credential for {registry} as {}, expires in {}s, cached for {}s",
        exchanged.username,
        exchanged.expires_in,
        cache.as_secs(),
    ));
    let response = CredentialProviderResponse::for_registry(
        registry,
        AuthConfig {
            username: exchanged.username,
            password: exchanged.access_token,
        },
        cache,
    );
    serde_json::to_string(&response)
        .map_err(|e| Error::Usage(format!("cannot encode response: {e}")))
}
