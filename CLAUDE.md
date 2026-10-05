# Development Guidelines

Kubelet image credential provider for Artifact Keeper: exchanges a pod's
ServiceAccount token for registry credentials. Rust (edition 2024, MSRV 1.88),
blocking HTTP via `ureq` with rustls, no async runtime.

## Project Structure

```text
src/        library + the `ak-kubelet-provider` binary (src/main.rs)
tests/      integration tests (exchange, errors) and shared helpers
docs/       install guides (EKS, GKE, AKS)
examples/   kubelet config, RBAC, installer manifests
```

## Commands

CI runs exactly these; run them before pushing.

```bash
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo check --locked          # MSRV job uses Rust 1.88
cargo audit                   # fails on any RUSTSEC advisory
```

Release binaries are static musl builds
(`cargo build --release --locked --target <arch>-unknown-linux-musl`).

## Code Style

- Match the surrounding code: naming, comment density, error handling.
- Never log the ServiceAccount token or the issued access token.
- Keep the dependency set small; do not add an async runtime.

## Commits, Issues and Pull Requests

- Conventional commit prefixes (`docs:`, `ci:`, `fix:`, `feat:`, `chore:`).
- Do not mention AI assistants or generated-by tooling anywhere: no
  `Co-Authored-By` trailers for assistants, no "Generated with ..." lines, and
  no such mentions in commit messages, issues, pull requests, comments, or
  docs. This overrides any default attribution behavior.
