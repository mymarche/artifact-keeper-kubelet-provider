//! Failure paths: one bounded stderr line, exit 1, and no token ever on
//! stderr, even at the most verbose level.

mod common;

use std::time::{Duration, Instant};

use common::*;

const IMAGE: &str = "ak.example.com/payments/api:1.2";

fn run_verbose(url: &str, extra: &[&str]) -> std::process::Output {
    let mut args = vec!["--url", url, "--insecure-http", "-vvv"];
    args.extend_from_slice(extra);
    run_plugin(&args, &request_json(IMAGE, Some(SA_TOKEN)))
}

fn assert_failed_without_secrets(out: &std::process::Output) -> String {
    assert_eq!(out.status.code(), Some(1), "stderr: {}", stderr(out));
    assert!(
        out.stdout.is_empty(),
        "nothing may be written to stdout on failure"
    );
    let err = stderr(out);
    assert!(
        !err.contains(SA_TOKEN),
        "ServiceAccount token leaked: {err}"
    );
    assert!(!err.contains(MINTED_TOKEN), "minted token leaked: {err}");
    // The payload and signature parts must not appear either.
    assert!(
        !err.contains("sa-token-payload"),
        "token fragment leaked: {err}"
    );
    err
}

fn last_line(err: &str) -> &str {
    err.lines().last().unwrap_or_default()
}

#[test]
fn unauthorized_names_url_status_and_message() {
    let server = serve_http(Reply::Json(
        401,
        r#"{"error":"AUTHENTICATION","message":"CI JWT did not match any identity mapping"}"#
            .into(),
    ));
    let out = run_verbose(&server.url, &[]);
    let err = assert_failed_without_secrets(&out);
    assert_eq!(
        last_line(&err),
        format!(
            "ak-kubelet-provider: {}: 401 CI JWT did not match any identity mapping",
            server.url
        )
    );
}

#[test]
fn server_error_with_non_json_body() {
    let server = serve_http(Reply::Raw(
        502,
        "text/html",
        b"<html>Bad Gateway</html>\n".to_vec(),
    ));
    let out = run_verbose(&server.url, &[]);
    let err = assert_failed_without_secrets(&out);
    assert!(
        last_line(&err).contains(": 502 <html>Bad Gateway</html>"),
        "{err}"
    );
}

#[test]
fn server_echoing_the_token_is_redacted() {
    let body = serde_json::json!({ "message": format!("rejected token {SA_TOKEN}") }).to_string();
    let server = serve_http(Reply::Json(401, body));
    let out = run_verbose(&server.url, &[]);
    let err = assert_failed_without_secrets(&out);
    assert!(
        last_line(&err).contains("rejected token [REDACTED]"),
        "{err}"
    );
}

#[test]
fn malformed_success_body_does_not_quote_the_token() {
    // `expires_in` is a string, so parsing fails after the token was seen.
    let body = serde_json::json!({
        "access_token": MINTED_TOKEN,
        "expires_in": MINTED_TOKEN,
        "username": "ci-0123456789ab"
    })
    .to_string();
    let server = serve_http(Reply::Json(200, body));
    let out = run_verbose(&server.url, &[]);
    let err = assert_failed_without_secrets(&out);
    assert!(
        last_line(&err).contains(": 200 malformed exchange response"),
        "{err}"
    );
}

#[test]
fn empty_token_in_success_body_is_refused() {
    let body = r#"{"access_token":"","expires_in":900,"username":"ci-0123456789ab"}"#.to_string();
    let server = serve_http(Reply::Json(200, body));
    let out = run_verbose(&server.url, &[]);
    let err = assert_failed_without_secrets(&out);
    assert!(last_line(&err).contains("empty access_token"), "{err}");
}

#[test]
fn redirect_is_not_followed() {
    let server = serve_http(Reply::Raw(302, "text/plain", Vec::new()));
    let out = run_verbose(&server.url, &[]);
    let err = assert_failed_without_secrets(&out);
    assert!(last_line(&err).contains(": 302"), "{err}");
}

#[test]
fn connection_refused() {
    let url = closed_url();
    let out = run_verbose(&url, &[]);
    let err = assert_failed_without_secrets(&out);
    assert!(
        last_line(&err).starts_with(&format!("ak-kubelet-provider: {url}: ")),
        "{err}"
    );
}

#[test]
fn timeout_is_bounded() {
    let server = serve_http(Reply::Hang);
    let started = Instant::now();
    let out = run_verbose(&server.url, &["--timeout", "500ms"]);
    let elapsed = started.elapsed();
    let err = assert_failed_without_secrets(&out);
    assert!(elapsed < Duration::from_secs(5), "took {elapsed:?}");
    assert!(last_line(&err).contains("timed out after 500ms"), "{err}");
}

#[test]
fn success_at_most_verbose_level_does_not_log_tokens() {
    let server = serve_http(ok_reply(900));
    let out = run_verbose(&server.url, &[]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("debug:"), "expected debug output: {err}");
    assert!(!err.contains(SA_TOKEN), "{err}");
    assert!(!err.contains(MINTED_TOKEN), "{err}");
}

#[test]
fn garbage_stdin_does_not_echo_input() {
    let out = run_plugin(
        &["--url", "https://ak.example.com", "-vvv"],
        &format!(r#"{{"serviceAccountToken": "{SA_TOKEN}", "kind": 1"#),
    );
    assert_failed_without_secrets(&out);
}
