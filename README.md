# artifact-keeper-kubelet-provider

A kubelet image credential provider for [Artifact Keeper](https://artifactkeeper.com).
Pods pull private images from Artifact Keeper **without `imagePullSecrets`**: the
kubelet hands the plugin the pulling pod's ServiceAccount token, the plugin
exchanges it at Artifact Keeper for a short-lived, pull-only credential, and the
kubelet uses that credential for the pull.

```
kubelet --(pod SA token, aud=artifact-keeper)--> ak-kubelet-provider
        <--(username + token, cacheDuration)----      |
                                                      | POST /api/v1/auth/ci/token
                                                      v
                                               Artifact Keeper
                                               verifies the token against the cluster's
                                               issuer keys, matches an identity mapping,
                                               mints a short-lived token
```

## Requirements

- **Kubernetes 1.34 or later.** The plugin relies on the kubelet supplying a
  ServiceAccount token (KEP-4412, `tokenAttributes`). Older clusters are not
  supported.
- **Artifact Keeper with a CI OIDC provider for the cluster**, and identity
  mappings for the ServiceAccounts that may pull:
  - **1.10.0 and later:** works today with a `generic` provider. See
    [Artifact Keeper 1.10: `generic` provider](#artifact-keeper-110-generic-provider)
    for the setup and its limits.
  - **Before 1.10.0:** the exchange needs the provider id, so set
    `--provider-id`. Otherwise the same setup applies.
  - A dedicated `kubernetes` provider type (pull-only token, no refresh token,
    namespace-level mappings, static keys for unreachable issuers) is planned in
    Artifact Keeper. It will make most of the limits below go away.
- Nodes that can reach Artifact Keeper over HTTPS.

## Install

1. Download the binary for the node architecture from the
   [releases](https://github.com/artifact-keeper/artifact-keeper-kubelet-provider/releases)
   and **verify it** before installing. It runs as root on every node:

   ```sh
   cosign verify-blob \
     --certificate ak-kubelet-provider-linux-amd64.pem \
     --signature ak-kubelet-provider-linux-amd64.sig \
     --certificate-identity "https://github.com/artifact-keeper/artifact-keeper-kubelet-provider/.github/workflows/release.yml@refs/tags/<tag>" \
     --certificate-oidc-issuer "https://token.actions.githubusercontent.com" \
     ak-kubelet-provider-linux-amd64
   sha256sum -c ak-kubelet-provider-linux-amd64.sha256
   ```

2. Install it as `ak-kubelet-provider` in the kubelet's credential provider
   bin dir, e.g. `/usr/libexec/kubernetes/kubelet-plugins/credential-provider/exec/`.
3. Write a `CredentialProviderConfig`. Start from
   [`examples/credential-provider-config.yaml`](examples/credential-provider-config.yaml).
4. Point the kubelet at both with `--image-credential-provider-config` and
   `--image-credential-provider-bin-dir`, then restart the kubelet.
5. **Allow nodes to request tokens for the audience** by applying
   [`examples/rbac.yaml`](examples/rbac.yaml), with `resources` set to your
   `serviceAccountTokenAudience`. Kubernetes refuses a node's token request for
   an audience it has not been granted (`ServiceAccountNodeAudienceRestriction`).
   Without this grant the kubelet logs
   `system:node:<node> is not authorized to request tokens for this audience`
   and then pulls **without credentials**, so the pull fails with a generic
   "pull access denied".

Platform-specific install guides (on-prem kubeadm, EKS, GKE, AKS) are in progress.

## Configuration

All configuration comes from `args` and `env` in the kubelet's
`CredentialProviderConfig`. Flags win over environment variables. There is no
config file and no state on the node.

| Flag              | Env               | Default | Meaning                                                              |
|-------------------|-------------------|---------|----------------------------------------------------------------------|
| `--url`           | `AK_URL`          | (none)  | Artifact Keeper base URL. Required, `https://` only.                 |
| `--provider-id`   | `AK_PROVIDER_ID`  | unset   | CI OIDC provider id, for clusters that share an issuer.              |
| `--ca-file`       | `AK_CA_FILE`      | unset   | Extra PEM CA bundle, trusted in addition to the Mozilla roots.       |
| `--timeout`       | `AK_TIMEOUT`      | `10s`   | Limit for the whole exchange (`ms`, `s`, `m`, `h`).                  |
| `--cache-margin`  | `AK_CACHE_MARGIN` | `60s`   | Subtracted from the token lifetime to get the kubelet cache duration.|
| `--log-level`     | `AK_LOG`          | `warn`  | `error`, `warn`, `info` or `debug`, written to stderr.               |
| `-v`              |                   |         | Raise the log level; repeat for more.                                |
| `--insecure-http` |                   | off     | Allow a plain `http://` URL. For testing only.                       |

`HTTPS_PROXY` and `NO_PROXY` are honoured. Redirects are never followed, so the
token is only ever sent to the configured URL.

### Audience

`tokenAttributes.serviceAccountTokenAudience` must equal the `audience` of the
cluster's provider in Artifact Keeper. Use a dedicated value such as
`artifact-keeper` or the instance URL, **never the API server's audience**. If
the two are the same, every pod's default token is accepted by Artifact Keeper,
and every token the plugin sends to Artifact Keeper is also valid against the
API server.

## Artifact Keeper 1.10: `generic` provider

Verified end to end on a kind 1.34.3 cluster against Artifact Keeper 1.10.0
running **inside the same cluster** with the kubeadm default issuer. That is the
hardest on-prem case. Managed clusters need fewer steps (see step 1).

### 1. Let Artifact Keeper reach the cluster's issuer

Artifact Keeper verifies the token by fetching
`{issuer}/.well-known/openid-configuration` and the `jwks_uri` it names. The
issuer is the `iss` of your ServiceAccount tokens:

```sh
kubectl create token default --duration 10m | cut -d. -f2 | base64 -d 2>/dev/null | grep -o '"iss":"[^"]*"'
```

- **EKS, GKE, AKS with the OIDC issuer enabled:** the issuer is public. Nothing
  to do.
- **On-prem, Artifact Keeper in the same cluster** (kubeadm default issuer
  `https://kubernetes.default.svc.cluster.local`):
  - allow anonymous discovery. By default only authenticated ServiceAccounts
    may read it:

    ```sh
    kubectl create clusterrolebinding oidc-discovery-anonymous \
      --clusterrole=system:service-account-issuer-discovery --group=system:unauthenticated
    ```

  - on Artifact Keeper, trust the cluster CA and allow private addresses. The
    issuer resolves to the ClusterIP, and `jwks_uri` points at the API server's
    node IP:

    ```yaml
    env:
      - {name: CUSTOM_CA_CERT_PATH, value: /var/run/secrets/kubernetes.io/serviceaccount/ca.crt}
      - {name: SSO_ALLOW_PRIVATE_IPS, value: "true"}
    ```

- **On-prem, Artifact Keeper outside the cluster:** the issuer URL itself must
  be reachable from Artifact Keeper. That means a cluster started with a
  `--service-account-issuer` that resolves from outside, plus the same discovery
  binding, CA and private-address settings. If that is not possible, wait for
  static keys in the `kubernetes` provider type.

### 2. Create the provider

```sh
curl -X POST https://ak.example.com/api/v1/admin/ci-oidc \
  -H "Authorization: Bearer $ADMIN_TOKEN" -H "Content-Type: application/json" \
  -d '{"name":"prod-cluster","provider_type":"generic",
       "issuer_url":"https://kubernetes.default.svc.cluster.local",
       "audience":"artifact-keeper"}'
```

`issuer_url` is the token's `iss` exactly. `audience` equals
`tokenAttributes.serviceAccountTokenAudience`.

### 3. One mapping per ServiceAccount

```sh
curl -X POST https://ak.example.com/api/v1/admin/ci-oidc/$PROVIDER_ID/mappings \
  -H "Authorization: Bearer $ADMIN_TOKEN" -H "Content-Type: application/json" \
  -d '{"name":"team-app","priority":10,
       "claim_filters":{"sub":"system:serviceaccount:team:app"}}'
```

Create **one mapping per ServiceAccount**, filtered on its exact `sub`. In 1.10
a mapping serves only the first `sub` that ever used it. A second ServiceAccount
admitted by the same mapping (for example through an any-of list) is refused
with `409 Username already exists` (artifact-keeper#4031). Whole-namespace
mappings are not possible yet.

### 4. Grant read access after the first pull

The mapping's service account (`ci-<8 hex of the mapping id>`, display name
`CI [<provider>] <mapping>`) is created by its **first** exchange. The first
pull therefore fails: Artifact Keeper answers `not found` so as not to reveal
the repository. Then:

1. find the account under Users (search `ci-`);
2. add it to a group that has **read-only** permission on the repositories to
   pull from (`POST /api/v1/groups/{id}/members`,
   `POST /api/v1/permissions` with `"actions":["read"]`).

The kubelet retries on its own, and the pull succeeds without restarting the
pod. Permissions are checked per request, so a credential it already cached
works too.

### Limits of this setup

- **Pull-only is up to you.** The minted token carries no action scope, so what
  it can do is exactly what the account's groups allow. Grant **read only**:
  with read only, a push with the node's credential is refused
  (`denied: You do not have access to this repository`). The same credential
  would push if a group granted write.
- The server's log attributes an exchange to the token's `sub` only, not to the
  pod or node.
- The token lifetime is Artifact Keeper's access-token lifetime, capped at the
  ServiceAccount token's own expiry.

## Caching

The kubelet caches the credential per registry and per ServiceAccount for
`cacheDuration`: the token lifetime minus `--cache-margin`. If the token would
expire within the margin, the response disables caching. With
`tokenAttributes.cacheType: ServiceAccount`, a second pod of the same
ServiceAccount reuses the cached credential, while a different ServiceAccount
triggers its own exchange.

## Troubleshooting

On failure the plugin exits 1 and writes one line to stderr. The kubelet logs it
(`Failed getting credential from external registry credential provider ...`)
and then pulls without credentials. The pod's events only show the generic
`pull access denied ... no basic auth credentials`, so **look in the kubelet
log on the node** (`journalctl -u kubelet | grep ak-kubelet-provider`). Tokens
are never printed, at any log level.

If the kubelet log has no line from the plugin at all, the plugin was never
run. Check `matchImages`, and look for a refused token request
(`not authorized to request tokens for this audience`, see Install step 5).

| Message                                                                 | Cause and fix |
|-------------------------------------------------------------------------|---------------|
| kubelet: `... is not authorized to request tokens for this audience`    | Nodes lack the audience grant. Apply `examples/rbac.yaml` with your audience. |
| `request has no serviceAccountToken; set tokenAttributes ...`           | The kubelet config for this provider has no `tokenAttributes`, or the kubelet is older than 1.34. |
| `unsupported apiVersion ...`                                            | The provider's `apiVersion` in `CredentialProviderConfig` is not `credentialprovider.kubelet.k8s.io/v1`. |
| `refusing to send a ServiceAccount token over plain http ...`           | `--url` is `http://`. Use `https://`. |
| `<url>: 401 CI JWT did not match any identity mapping`                  | No enabled mapping matches this ServiceAccount. Check the mapping's `claim_filters` (`sub` is `system:serviceaccount:<ns>:<name>`). |
| `<url>: 409 Username already exists`                                    | Artifact Keeper 1.10: the mapping already serves another ServiceAccount. Use one mapping per ServiceAccount. |
| `<url>: 404 No enabled CI OIDC provider is configured for issuer ...`   | No provider for this cluster's issuer, or it is disabled. `issuer_url` must equal the token's `iss`. |
| containerd: `not found` for an image that exists                        | The account has no read permission yet. Add its `ci-…` account to a read-only group (see step 4 above). |
| Any exchange error while the Artifact Keeper log shows a failed discovery or JWKS fetch | Artifact Keeper cannot reach the issuer. Check step 1 above. |
| `<url>: 400 ... supply provider_id to choose one`                       | Several providers share this cluster's issuer. Set `--provider-id`, or give the cluster a unique `--service-account-issuer`. |
| `<url>: 401 CI JWT validation failed ...`                               | Signature, audience or expiry check failed. Check that `serviceAccountTokenAudience` equals the provider's audience, and for static-key providers that the JWKS holds the cluster's current key. |
| `<url>: timed out after 10s`                                            | Artifact Keeper is unreachable from the node, or a proxy is needed (`HTTPS_PROXY`). |
| `<url>: ... invalid peer certificate ...`                               | Artifact Keeper's certificate is not trusted. Pass its CA with `--ca-file`. |

Run the plugin by hand to test a node's setup:

```sh
echo '{"apiVersion":"credentialprovider.kubelet.k8s.io/v1","kind":"CredentialProviderRequest","image":"ak.example.com/team/app:1.0","serviceAccountToken":"'"$(kubectl create token -n team app --audience artifact-keeper)"'"}' \
  | ak-kubelet-provider --url https://ak.example.com -v
```

## Building

```sh
cargo test
cargo build --release --target x86_64-unknown-linux-musl   # static binary
```

Release builds are static musl binaries for `linux/amd64` and `linux/arm64`,
signed with cosign keyless and shipped with a CycloneDX SBOM.

## License

MIT
