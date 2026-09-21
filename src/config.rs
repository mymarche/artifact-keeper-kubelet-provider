//! Configuration from flags and environment only (design D3). Flags win over
//! env; there is no config file.

use std::path::PathBuf;
use std::time::Duration;

use clap::{ArgAction, Parser};

use crate::error::Error;
use crate::log::Level;

#[derive(Parser, Debug)]
#[command(
    name = "ak-kubelet-provider",
    version,
    about = "Kubelet image credential provider for Artifact Keeper",
    long_about = "Kubelet image credential provider for Artifact Keeper.\n\n\
                  Reads a CredentialProviderRequest (credentialprovider.kubelet.k8s.io/v1) \
                  on stdin, exchanges the pod's ServiceAccount token at Artifact Keeper, and \
                  writes a CredentialProviderResponse on stdout. Requires Kubernetes 1.34+ \
                  with tokenAttributes set for this provider."
)]
pub struct Config {
    /// Artifact Keeper base URL, e.g. https://ak.example.com
    #[arg(long, env = "AK_URL")]
    pub url: String,

    /// CI OIDC provider id, for clusters that share an issuer
    #[arg(long, env = "AK_PROVIDER_ID")]
    pub provider_id: Option<String>,

    /// Extra PEM CA bundle, trusted in addition to the Mozilla roots
    #[arg(long, env = "AK_CA_FILE")]
    pub ca_file: Option<PathBuf>,

    /// Limit for the whole exchange, e.g. 10s or 500ms
    #[arg(long, env = "AK_TIMEOUT", default_value = "10s", value_parser = parse_duration)]
    pub timeout: Duration,

    /// Subtracted from the token lifetime to get the kubelet cache duration
    #[arg(long, env = "AK_CACHE_MARGIN", default_value = "60s", value_parser = parse_duration)]
    pub cache_margin: Duration,

    /// Base log level on stderr: error, warn, info or debug
    #[arg(long, env = "AK_LOG", default_value = "warn", value_parser = parse_level)]
    pub log_level: Level,

    /// Raise the log level; repeat for more
    #[arg(short = 'v', action = ArgAction::Count)]
    pub verbose: u8,

    /// Allow a plain http:// URL. For testing only.
    #[arg(long)]
    pub insecure_http: bool,
}

impl Config {
    /// Parse from the process arguments and environment, reporting any
    /// problem as a one-line usage error. `--help` and `--version` print and
    /// exit as usual.
    pub fn from_env() -> Result<Self, Error> {
        match Self::try_parse() {
            Ok(config) => config.validated(),
            Err(e) if !e.use_stderr() => e.exit(),
            Err(e) => Err(Error::Usage(first_line(&e.to_string()))),
        }
    }

    pub fn validated(mut self) -> Result<Self, Error> {
        self.url = self.url.trim().trim_end_matches('/').to_owned();
        let lower = self.url.to_ascii_lowercase();
        let host = if let Some(rest) = lower.strip_prefix("https://") {
            rest
        } else if let Some(rest) = lower.strip_prefix("http://") {
            if !self.insecure_http {
                return Err(Error::Usage(format!(
                    "refusing to send a ServiceAccount token over plain http ({}); use https",
                    self.url
                )));
            }
            rest
        } else {
            return Err(Error::Usage(format!(
                "--url must be an https:// URL, got {:?}",
                self.url
            )));
        };
        if host.is_empty() || host.starts_with('/') {
            return Err(Error::Usage(format!("--url has no host: {:?}", self.url)));
        }
        if host.split('/').next().is_some_and(|h| h.contains('@')) {
            return Err(Error::Usage("--url must not contain credentials".into()));
        }
        if let Some(id) = &self.provider_id {
            let id = id.trim();
            if id.is_empty() {
                self.provider_id = None;
            } else {
                self.provider_id = Some(id.to_owned());
            }
        }
        if self.timeout.is_zero() {
            return Err(Error::Usage("--timeout must be greater than zero".into()));
        }
        Ok(self)
    }

    pub fn effective_level(&self) -> Level {
        self.log_level.raised_by(self.verbose)
    }
}

fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("invalid arguments")
        .trim_start_matches("error: ")
        .to_owned()
}

/// Parse `<number><unit>` with unit `ms`, `s`, `m` or `h`.
pub fn parse_duration(text: &str) -> Result<Duration, String> {
    let text = text.trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .ok_or_else(|| format!("{text:?} has no unit; use ms, s, m or h"))?;
    let (number, unit) = text.split_at(split);
    let n: u64 = number
        .parse()
        .map_err(|_| format!("{text:?} is not a duration"))?;
    let secs = |mul: u64| {
        n.checked_mul(mul)
            .map(Duration::from_secs)
            .ok_or_else(|| format!("{text:?} is too large"))
    };
    match unit {
        "ms" => Ok(Duration::from_millis(n)),
        "s" => secs(1),
        "m" => secs(60),
        "h" => secs(3600),
        _ => Err(format!(
            "{text:?} has unknown unit {unit:?}; use ms, s, m or h"
        )),
    }
}

fn parse_level(text: &str) -> Result<Level, String> {
    text.parse()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Config, Error> {
        let mut argv = vec!["ak-kubelet-provider"];
        argv.extend_from_slice(args);
        Config::try_parse_from(argv)
            .map_err(|e| Error::Usage(first_line(&e.to_string())))?
            .validated()
    }

    #[test]
    fn defaults() {
        let c = parse(&["--url", "https://ak.example.com/"]).unwrap();
        assert_eq!(c.url, "https://ak.example.com");
        assert_eq!(c.provider_id, None);
        assert_eq!(c.timeout, Duration::from_secs(10));
        assert_eq!(c.cache_margin, Duration::from_secs(60));
        assert_eq!(c.effective_level(), Level::Warn);
    }

    #[test]
    fn url_is_required() {
        // Only meaningful when AK_URL is not set in the test environment.
        if std::env::var_os("AK_URL").is_none() {
            assert!(parse(&[]).is_err());
        }
    }

    #[test]
    fn plain_http_is_refused_without_flag() {
        let err = parse(&["--url", "http://ak.example.com"]).err().unwrap();
        assert!(err.to_string().contains("plain http"), "{err}");
        assert!(parse(&["--url", "http://ak.example.com", "--insecure-http"]).is_ok());
    }

    #[test]
    fn other_schemes_and_credentials_are_refused() {
        assert!(parse(&["--url", "ftp://ak.example.com"]).is_err());
        assert!(parse(&["--url", "ak.example.com"]).is_err());
        assert!(parse(&["--url", "https://"]).is_err());
        assert!(parse(&["--url", "https://user:pw@ak.example.com"]).is_err());
    }

    #[test]
    fn blank_provider_id_is_unset() {
        let c = parse(&["--url", "https://a.example", "--provider-id", "  "]).unwrap();
        assert_eq!(c.provider_id, None);
    }

    #[test]
    fn verbose_raises_level() {
        let c = parse(&["--url", "https://a.example", "-vv"]).unwrap();
        assert_eq!(c.effective_level(), Level::Debug);
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
        assert_eq!(parse_duration("10s").unwrap(), Duration::from_secs(10));
        assert_eq!(parse_duration("2m").unwrap(), Duration::from_secs(120));
        assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
        assert!(parse_duration("10").is_err());
        assert!(parse_duration("s").is_err());
        assert!(parse_duration("10d").is_err());
    }

    #[test]
    fn zero_timeout_is_refused() {
        assert!(parse(&["--url", "https://a.example", "--timeout", "0s"]).is_err());
    }
}
