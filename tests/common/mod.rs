//! A minimal HTTP/1.1 mock of Artifact Keeper's exchange endpoint, plain or
//! TLS, and a helper that runs the real binary against it.

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

pub const SA_TOKEN: &str = "eyJhbGciOiJSUzI1NiJ9.sa-token-payload.sa-token-signature";
pub const MINTED_TOKEN: &str = "eyJhbGciOiJIUzI1NiJ9.minted-payload.minted-signature";

pub fn request_json(image: &str, token: Option<&str>) -> String {
    let mut v = serde_json::json!({
        "kind": "CredentialProviderRequest",
        "apiVersion": "credentialprovider.kubelet.k8s.io/v1",
        "image": image,
    });
    if let Some(t) = token {
        v["serviceAccountToken"] = serde_json::Value::String(t.into());
    }
    v.to_string()
}

pub struct Recorded {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

pub enum Reply {
    Json(u16, String),
    Raw(u16, &'static str, Vec<u8>),
    /// Accept the connection and never answer.
    Hang,
}

pub fn ok_reply(expires_in: u64) -> Reply {
    Reply::Json(
        200,
        serde_json::json!({
            "access_token": MINTED_TOKEN,
            "token_type": "Bearer",
            "expires_in": expires_in,
            "username": "ci-0123456789ab",
        })
        .to_string(),
    )
}

pub struct MockServer {
    pub url: String,
    pub requests: Receiver<Recorded>,
}

/// Serve one connection over plain HTTP.
pub fn serve_http(reply: Reply) -> MockServer {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            handle(stream, reply, &tx);
        }
    });
    MockServer { url, requests: rx }
}

/// Serve one connection over TLS with `cert` as the server identity.
pub fn serve_tls(cert: &TestPki, reply: Reply) -> MockServer {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "https://localhost:{}",
        listener.local_addr().unwrap().port()
    );
    let config = cert.server_config();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        if let Ok((tcp, _)) = listener.accept() {
            let conn = rustls::ServerConnection::new(config).unwrap();
            let mut tls = rustls::StreamOwned::new(conn, tcp);
            // A handshake failure is the expected outcome of some tests.
            if tls.conn.complete_io(&mut tls.sock).is_ok() {
                handle(tls, reply, &tx);
            }
        }
    });
    MockServer { url, requests: rx }
}

fn handle<S: Read + Write>(stream: S, reply: Reply, tx: &mpsc::Sender<Recorded>) {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let path = parts.next().unwrap_or_default().to_owned();
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
            break;
        }
        if let Some((k, v)) = h.trim_end().split_once(':') {
            headers.push((k.trim().to_owned(), v.trim().to_owned()));
        }
    }
    let len = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0; len];
    reader.read_exact(&mut body).ok();
    tx.send(Recorded {
        method,
        path,
        headers,
        body,
    })
    .ok();

    let (status, content_type, payload) = match reply {
        Reply::Json(s, b) => (s, "application/json", b.into_bytes()),
        Reply::Raw(s, ct, b) => (s, ct, b),
        Reply::Hang => {
            thread::sleep(Duration::from_secs(30));
            return;
        }
    };
    let mut stream = reader.into_inner();
    let head = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    stream.write_all(head.as_bytes()).ok();
    stream.write_all(&payload).ok();
    stream.flush().ok();
}

/// A free port with nothing listening on it.
pub fn closed_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    // Make sure the port is really closed before handing it out.
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
    format!("http://127.0.0.1:{port}")
}

/// Run the binary with `args`, `input` on stdin and a clean AK_* environment.
pub fn run_plugin(args: &[&str], input: &str) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ak-kubelet-provider"));
    for (k, _) in std::env::vars() {
        if k.starts_with("AK_")
            || k.eq_ignore_ascii_case("https_proxy")
            || k.eq_ignore_ascii_case("http_proxy")
            || k.eq_ignore_ascii_case("all_proxy")
        {
            cmd.env_remove(k);
        }
    }
    let mut child = cmd
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

pub fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

pub fn stdout_json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON ({e}); stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

/// A test CA and a `localhost` server certificate it issued.
pub struct TestPki {
    pub ca_pem: String,
    server_cert_der: Vec<u8>,
    server_key_der: Vec<u8>,
}

impl TestPki {
    pub fn new() -> Self {
        use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};

        let ca_key = KeyPair::generate().unwrap();
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca_cert = ca_params.self_signed(&ca_key).unwrap();
        let issuer = Issuer::new(ca_params, ca_key);

        let server_key = KeyPair::generate().unwrap();
        let server_params = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        let server_cert = server_params.signed_by(&server_key, &issuer).unwrap();

        Self {
            ca_pem: ca_cert.pem(),
            server_cert_der: server_cert.der().to_vec(),
            server_key_der: server_key.serialize_der(),
        }
    }

    fn server_config(&self) -> Arc<rustls::ServerConfig> {
        use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(self.server_cert_der.clone())],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.server_key_der.clone())),
            )
            .unwrap();
        Arc::new(config)
    }

    pub fn write_ca(&self, name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("akkp-{name}-{}.pem", std::process::id()));
        std::fs::write(&path, &self.ca_pem).unwrap();
        path
    }
}
