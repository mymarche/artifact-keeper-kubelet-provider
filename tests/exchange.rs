//! The real binary against a mock Artifact Keeper: request shape, response
//! mapping, TLS trust, and plain-http refusal.

mod common;

use std::time::Duration;

use common::*;

const IMAGE: &str = "ak.example.com:5000/payments/api:1.2";

#[test]
fn exchange_without_provider_id_sends_bearer_and_no_body() {
    let server = serve_http(ok_reply(900));
    let out = run_plugin(
        &["--url", &server.url, "--insecure-http"],
        &request_json(IMAGE, Some(SA_TOKEN)),
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));

    let req = server
        .requests
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    assert_eq!(req.method, "POST");
    assert_eq!(req.path, "/api/v1/auth/ci/token");
    assert_eq!(
        req.header("authorization"),
        Some(format!("Bearer {SA_TOKEN}").as_str())
    );
    assert!(
        req.body.is_empty(),
        "body: {:?}",
        String::from_utf8_lossy(&req.body)
    );
}

#[test]
fn exchange_with_provider_id_sends_json_body() {
    let server = serve_http(ok_reply(900));
    let out = run_plugin(
        &[
            "--url",
            &format!("{}/", server.url),
            "--insecure-http",
            "--provider-id",
            "8f1c2d3e-0000-4000-8000-000000000001",
        ],
        &request_json(IMAGE, Some(SA_TOKEN)),
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));

    let req = server
        .requests
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    assert_eq!(req.path, "/api/v1/auth/ci/token");
    assert_eq!(req.header("content-type"), Some("application/json"));
    let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(
        body,
        serde_json::json!({"provider_id": "8f1c2d3e-0000-4000-8000-000000000001"})
    );
}

#[test]
fn response_maps_to_registry_credential() {
    let server = serve_http(ok_reply(900));
    let out = run_plugin(
        &["--url", &server.url, "--insecure-http"],
        &request_json(IMAGE, Some(SA_TOKEN)),
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(
        stdout_json(&out),
        serde_json::json!({
            "kind": "CredentialProviderResponse",
            "apiVersion": "credentialprovider.kubelet.k8s.io/v1",
            "cacheKeyType": "Registry",
            "cacheDuration": "840s",
            "auth": {
                "ak.example.com:5000": {"username": "ci-0123456789ab", "password": MINTED_TOKEN}
            }
        })
    );
}

#[test]
fn nearly_expired_token_disables_caching() {
    let server = serve_http(ok_reply(45));
    let out = run_plugin(
        &["--url", &server.url, "--insecure-http"],
        &request_json(IMAGE, Some(SA_TOKEN)),
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout_json(&out)["cacheDuration"], "0s");
}

#[test]
fn plain_http_is_refused_before_sending() {
    let server = serve_http(ok_reply(900));
    let out = run_plugin(
        &["--url", &server.url],
        &request_json(IMAGE, Some(SA_TOKEN)),
    );
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("plain http"),
        "stderr: {}",
        stderr(&out)
    );
    assert!(
        server
            .requests
            .recv_timeout(Duration::from_millis(300))
            .is_err(),
        "the token must not be sent"
    );
}

#[test]
fn missing_service_account_token_makes_no_request() {
    let server = serve_http(ok_reply(900));
    let out = run_plugin(
        &["--url", &server.url, "--insecure-http"],
        &request_json(IMAGE, None),
    );
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("tokenAttributes"),
        "stderr: {}",
        stderr(&out)
    );
    assert!(
        server
            .requests
            .recv_timeout(Duration::from_millis(300))
            .is_err()
    );
}

#[test]
fn unsupported_api_version_is_refused() {
    let input = request_json(IMAGE, Some(SA_TOKEN)).replace(
        "credentialprovider.kubelet.k8s.io/v1",
        "credentialprovider.kubelet.k8s.io/v1beta1",
    );
    let out = run_plugin(&["--url", "https://ak.example.com"], &input);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("credentialprovider.kubelet.k8s.io/v1"),
        "stderr: {}",
        stderr(&out)
    );
}

#[test]
fn env_configures_and_flags_win() {
    let server = serve_http(ok_reply(900));
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_ak-kubelet-provider"));
    cmd.env("AK_URL", "https://wrong.invalid")
        .env("AK_PROVIDER_ID", "from-env")
        .env("AK_CACHE_MARGIN", "100s")
        .args(["--url", &server.url, "--insecure-http"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(request_json(IMAGE, Some(SA_TOKEN)).as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout_json(&out)["cacheDuration"], "800s");
    let req = server
        .requests
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(body["provider_id"], "from-env");
}

#[test]
fn tls_with_private_ca_file_succeeds() {
    let pki = TestPki::new();
    let ca = pki.write_ca("ok");
    let server = serve_tls(&pki, ok_reply(900));
    let out = run_plugin(
        &["--url", &server.url, "--ca-file", ca.to_str().unwrap()],
        &request_json(IMAGE, Some(SA_TOKEN)),
    );
    std::fs::remove_file(&ca).ok();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(
        stdout_json(&out)["auth"]["ak.example.com:5000"]["password"],
        MINTED_TOKEN
    );
    let req = server
        .requests
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    assert_eq!(req.path, "/api/v1/auth/ci/token");
}

#[test]
fn tls_without_ca_file_is_refused() {
    let pki = TestPki::new();
    let server = serve_tls(&pki, ok_reply(900));
    let out = run_plugin(
        &["--url", &server.url],
        &request_json(IMAGE, Some(SA_TOKEN)),
    );
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains(&server.url), "stderr: {err}");
    assert!(!err.contains(SA_TOKEN), "stderr: {err}");
    assert!(
        server
            .requests
            .recv_timeout(Duration::from_millis(300))
            .is_err(),
        "no request may complete over an untrusted connection"
    );
}
