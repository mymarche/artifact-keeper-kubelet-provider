use std::fmt;

/// Longest server-supplied message the plugin repeats on stderr.
const MAX_SERVER_MESSAGE: usize = 200;

/// A failure reported to the kubelet as one stderr line and exit status 1.
#[derive(Debug)]
pub enum Error {
    /// Unusable configuration or request. Nothing was sent to Artifact Keeper.
    Usage(String),
    /// The exchange with Artifact Keeper failed.
    Exchange {
        url: String,
        status: Option<u16>,
        message: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Usage(message) => f.write_str(message),
            Error::Exchange {
                url,
                status: Some(status),
                message,
            } => write!(f, "{url}: {status} {message}"),
            Error::Exchange {
                url,
                status: None,
                message,
            } => write!(f, "{url}: {message}"),
        }
    }
}

impl std::error::Error for Error {}

/// Make text from an untrusted source safe for a single log line: control
/// characters become spaces, every occurrence of `secret` is redacted, and the
/// result is truncated to a fixed length.
pub fn sanitize(text: &str, secret: &str) -> String {
    let scrubbed = if secret.is_empty() {
        text.to_owned()
    } else {
        text.replace(secret, "[REDACTED]")
    };
    let mut out: String = scrubbed
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if out.chars().count() > MAX_SERVER_MESSAGE {
        out = out.chars().take(MAX_SERVER_MESSAGE).collect();
        out.push_str("...");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exchange_error_names_url_status_and_message() {
        let e = Error::Exchange {
            url: "https://ak.example.com".into(),
            status: Some(401),
            message: "CI JWT did not match any identity mapping".into(),
        };
        assert_eq!(
            e.to_string(),
            "https://ak.example.com: 401 CI JWT did not match any identity mapping"
        );
    }

    #[test]
    fn exchange_error_without_status() {
        let e = Error::Exchange {
            url: "https://ak.example.com".into(),
            status: None,
            message: "timed out after 10s".into(),
        };
        assert_eq!(e.to_string(), "https://ak.example.com: timed out after 10s");
    }

    #[test]
    fn sanitize_flattens_control_characters() {
        assert_eq!(sanitize("line one\nline\ttwo\r\n", ""), "line one line two");
    }

    #[test]
    fn sanitize_redacts_the_secret() {
        assert_eq!(
            sanitize("bad token eyJabc.def.ghi here", "eyJabc.def.ghi"),
            "bad token [REDACTED] here"
        );
    }

    #[test]
    fn sanitize_truncates_long_messages() {
        let long = "x".repeat(MAX_SERVER_MESSAGE + 50);
        let out = sanitize(&long, "");
        assert_eq!(out.chars().count(), MAX_SERVER_MESSAGE + 3);
        assert!(out.ends_with("..."));
    }
}
