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
  - **1.11.0 and later:** use a `kubernetes` provider. It mints pull-only
    credentials and supports namespace-wide mappings and static keys. See
    [Artifact Keeper 1.11: `kubernetes` provider](#artifact-keeper-111-kubernetes-provider).
  - **1.10.x:** works with a `generic` provider, with more limits. See
    [Artifact Keeper 1.10: `generic` provider](#artifact-keeper-110-generic-provider).
  - **Before 1.10.0:** the exchange needs the provider id, so set
    `--provider-id`. Otherwise the same setup as 1.10 applies.
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

These steps are the reference for nodes whose kubelet you configure yourself
(kubeadm and other self-managed clusters). For managed clusters:

- **Amazon EKS** (AL2023 nodes): [docs/install-eks.md](docs/install-eks.md),
  through `nodeadm` user data, next to `ecr-credential-provider`.
- **GKE Standard and AKS**: [docs/install-gke-aks.md](docs/install-gke-aks.md),
  through a privileged installer DaemonSet. **Not supported by Google or
  Microsoft.**

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
API server. Artifact Keeper 1.11 refuses to create a provider with the API
server's audience.

## Artifact Keeper 1.11: `kubernetes` provider

Artifact Keeper 1.11.0 has a `kubernetes` CI OIDC provider type made for this
plugin (artifact-keeper#1246; reference:
[`docs/ci-oidc.md`](https://github.com/artifact-keeper/artifact-keeper/blob/main/docs/ci-oidc.md)).
Compared with `generic` it:

- mints a **pull-only** credential. The token carries only the
  `read:artifacts` and `read:repositories` scopes, whatever the account's groups
  allow. A push with it is refused (`denied: Token does not have required scope:
  write:artifacts`);
- mints no refresh token, and the credential cannot create API tokens (`403`);
- takes static signing keys for clusters whose issuer Artifact Keeper cannot
  reach;
- lets one mapping cover a whole namespace;
- logs the namespace, ServiceAccount, pod and node of every exchange.

Verified end to end on a kind 1.34.3 cluster against Artifact Keeper 1.11.1
running **inside the same cluster**, with the kubeadm default issuer and static
keys. Not yet run: the `discovery` key source and real EKS, GKE and AKS
clusters.

### 1. Create the provider

The `issuer_url` is the `iss` of your ServiceAccount tokens, which the cluster
reports itself:

```sh
kubectl get --raw /.well-known/openid-configuration | jq -r .issuer
```

| Cluster                                        | `key_source` |
|------------------------------------------------|--------------|
| EKS, GKE, AKS with the OIDC issuer enabled     | `discovery`: Artifact Keeper fetches the public keys itself |
| AKS without the OIDC issuer, on-prem (kubeadm) | `static`: you give Artifact Keeper the cluster's public keys |

With `static`, Artifact Keeper never contacts the issuer. That is what lets an
on-prem or in-cluster Artifact Keeper work without opening the API server's
discovery endpoint:

```sh
kubectl get --raw /openid/v1/jwks > jwks.json
jq -n --slurpfile jwks jwks.json '{
      name: "prod-cluster", provider_type: "kubernetes",
      issuer_url: "https://kubernetes.default.svc.cluster.local",
      audience: "artifact-keeper",
      key_source: "static", static_jwks: $jwks[0]}' |
  curl -X POST https://ak.example.com/api/v1/admin/ci-oidc \
    -H "Authorization: Bearer $ADMIN_TOKEN" -H "Content-Type: application/json" -d @-
```

For `discovery`, leave out `key_source` and `static_jwks` (or set
`"key_source": "discovery"`). `audience` equals
`tokenAttributes.serviceAccountTokenAudience`. Artifact Keeper refuses the API
server's own audience (`https://kubernetes.default.svc`) with `400`.

### 2. Grant read access, then map ServiceAccounts

Create a group with **read** permission on the repositories the pods pull from,
and bind the mapping to it with `group_binding_ids`:

```sh
curl -X POST https://ak.example.com/api/v1/groups \
  -H "Authorization: Bearer $ADMIN_TOKEN" -H "Content-Type: application/json" \
  -d '{"name":"k8s-pullers"}'
curl -X POST https://ak.example.com/api/v1/permissions \
  -H "Authorization: Bearer $ADMIN_TOKEN" -H "Content-Type: application/json" \
  -d '{"principal_type":"group","principal_id":"'$GROUP_ID'",
       "target_type":"repository","target_id":"'$REPO_ID'","actions":["read"]}'
curl -X POST https://ak.example.com/api/v1/admin/ci-oidc/$PROVIDER_ID/mappings \
  -H "Authorization: Bearer $ADMIN_TOKEN" -H "Content-Type: application/json" \
  -d '{"name":"team-ns","priority":10,
       "claim_filters":{"/kubernetes.io/namespace":"team"},
       "group_binding_ids":["'$GROUP_ID'"]}'
```

Keys that start with `/` are JSON Pointers into the token's claims.
`{"/kubernetes.io/namespace":"team"}` admits every ServiceAccount in `team`
(exact match: `team` does not admit `team-sandbox`). To admit one ServiceAccount,
add `"/kubernetes.io/serviceaccount/name":"app"`, or filter on
`"sub":"system:serviceaccount:team:app"`.

The mapping's service account (`ci-<12 hex>`) exists as soon as the mapping does,
and the group binding gives it read access, so **the first pull works**. All
ServiceAccounts admitted by one mapping share that account. The server log
tells them apart by `k8s_namespace`, `k8s_serviceaccount`, `k8s_pod` and
`k8s_node`.

The mapping is the security boundary: everything it admits holds the same
access. Keep mappings as narrow as the trust between the workloads.

### Limits of this setup

- The token lifetime is Artifact Keeper's access-token lifetime, capped at the
  ServiceAccount token's own expiry.
- The kubelet caches the credential on the node, where root can read it. That is
  why the pull-only ceiling is fixed by the provider type.
- With `static` keys, a rotated cluster signing key must reach the provider
  before tokens signed with it arrive. Tokens with an unknown `kid` are refused
  with `401`. Rotate with overlap, as the Artifact Keeper docs describe.
- Rolling Artifact Keeper back below 1.11 turns a `kubernetes` provider into
  `generic` (unrestricted, renewable tokens). Disable these providers first.

## Artifact Keeper 1.10: `generic` provider

Use this only with Artifact Keeper 1.10.x. From 1.11.0, the `kubernetes` provider
above replaces it and removes most of the limits listed here.

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
| `<url>: 401 CI JWT did not match any identity mapping`                  | No enabled mapping matches this ServiceAccount. Check the mapping's `claim_filters`: `/kubernetes.io/namespace` and `/kubernetes.io/serviceaccount/name` for a `kubernetes` provider, `sub` (`system:serviceaccount:<ns>:<name>`) for either type. |
| `<url>: 409 Username already exists`                                    | Artifact Keeper 1.10 with a `generic` provider: the mapping already serves another ServiceAccount. Use one mapping per ServiceAccount, or move to 1.11 and a `kubernetes` provider. |
| `<url>: 404 No enabled CI OIDC provider is configured for issuer ...`   | No provider for this cluster's issuer, or it is disabled. `issuer_url` must equal the token's `iss`. |
| containerd: `not found` for an image that exists                        | The account has no read permission. `kubernetes` provider: the mapping has no `group_binding_ids`, or the group has no `read` on the repository. `generic` provider: add its `ci-…` account to a read-only group (see step 4 of the 1.10 setup). |
| Any exchange error while the Artifact Keeper log shows a failed discovery or JWKS fetch | Artifact Keeper cannot reach the issuer. Use `key_source: static` on a `kubernetes` provider (1.11), or see step 1 of the 1.10 setup. |
| `<url>: 400 ... supply provider_id to choose one`                       | Several providers share this cluster's issuer. Set `--provider-id`, or give the cluster a unique `--service-account-issuer`. |
| `<url>: 401 CI JWT validation failed ...`                               | Signature, audience or expiry check failed. Check that `serviceAccountTokenAudience` equals the provider's audience, and for static-key providers that the JWKS holds the cluster's current key (the Artifact Keeper log names a missing `kid`). |
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
