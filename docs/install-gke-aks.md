# Install on GKE and AKS (unsupported by the vendor)

> **Google and Microsoft do not support this.** Neither GKE nor AKS lets you
> add a kubelet image credential provider through its API. This guide uses a
> privileged DaemonSet that edits the kubelet's configuration on each node and
> restarts the kubelet. That is a change to the node outside the vendor's API,
> and the vendor can revert it or break it with a node image or agent update.
> Use it only if you accept that. The alternative is `imagePullSecrets`.

The installer is [`examples/installer/daemonset.yaml`](../examples/installer/daemonset.yaml).
It uses no platform-specific paths. It reads `--image-credential-provider-config`
and `--image-credential-provider-bin-dir` from the running kubelet and adds
itself next to the provider the platform already configured:

| Platform | Existing provider         | Config (as found on nodes)                        | Bin dir                             |
|----------|---------------------------|---------------------------------------------------|-------------------------------------|
| GKE      | `auth-provider-gcp`       | `/etc/srv/kubernetes/cri_auth_config.yaml`        | `/home/kubernetes/bin`              |
| AKS      | `acr-credential-provider` | `/var/lib/kubelet/credential-provider-config.yaml`| `/var/lib/kubelet/credential-provider` |

If the kubelet has neither flag, the installer changes nothing and reports it.

> **Verification status.** Tested on a Kubernetes 1.34.3 kind node set up like
> an AKS node: an existing provider entry, config and bin dir at non-default
> paths. Tested there: install and a pull without `imagePullSecrets`; no
> changes on later passes; restoring the entry after the file was replaced;
> rolling back a config the kubelet refused to start with; uninstall, which left
> the original file byte for byte. **Not yet run on real GKE or AKS clusters.**
> Report the result, good or bad, in an issue.

## Where it applies

| Cluster                                   | Supported |
|-------------------------------------------|-----------|
| GKE Standard, Container-Optimized OS or Ubuntu node pools | yes, vendor-unsupported |
| AKS, Ubuntu or Azure Linux node pools     | yes, vendor-unsupported |
| GKE Autopilot                             | no: privileged pods and host paths are not allowed |
| AKS Automatic, AKS virtual nodes          | no: no access to the node |
| Windows node pools                        | no: the DaemonSet only runs on Linux nodes |

The control plane and the nodes must run Kubernetes **1.34 or later**.

## What the installer does

On each node, every `RECONCILE_INTERVAL` seconds (default 300):

1. Finds the kubelet's credential provider config and bin dir from its
   command line.
2. Downloads `ak-kubelet-provider-linux-<arch>` from `BINARY_BASE_URL` if the
   installed binary differs from the pinned SHA-256. It installs the binary
   only if the checksum matches.
3. Adds the `ak-kubelet-provider` entry from the ConfigMap to the config file,
   or updates it there. Other providers and comments are kept.
4. If the file changed, restarts the kubelet and waits up to 60 s for
   `http://127.0.0.1:10248/healthz`. If the kubelet does not come up, it puts
   the previous file back, restarts the kubelet again, records the rejected
   config in `<config>.ak-failed`, and does not retry that exact config.
5. Marks the pod Ready only when the pass succeeded.

Repeating the pass covers the cases where the platform replaces the node's
state: `/etc` on Container-Optimized OS is reset on reboot, and node images and
agents can rewrite the config. New nodes from upgrades, auto-repair or
autoscaling get the plugin as soon as the DaemonSet pod starts there. Until
then, pulls from Artifact Keeper on that node fail and are retried with the
usual back-off.

A kubelet restart does not stop running pods. The DaemonSet updates one node at
a time and waits 60 s on each (`minReadySeconds`), so a config that breaks the
kubelet stops the rollout after one node.

## Steps

### 1. Artifact Keeper: a provider for the cluster

Get the cluster's ServiceAccount issuer:

```sh
kubectl get --raw /.well-known/openid-configuration | jq -r .issuer
```

- **GKE:** the issuer is
  `https://container.googleapis.com/v1/projects/<project>/locations/<location>/clusters/<cluster>`,
  and its discovery document and keys are public.
- **AKS:** enable the OIDC issuer so that its keys are public:

  ```sh
  az aks update --resource-group <rg> --name <cluster> --enable-oidc-issuer
  az aks show --resource-group <rg> --name <cluster> --query oidcIssuerProfile.issuerUrl -o tsv
  ```

  Enabling it changes the issuer of newly minted tokens. Without it, Artifact
  Keeper cannot fetch the cluster's keys, and 1.10 has no static keys, so this
  setup needs the OIDC issuer enabled.

Create a CI OIDC provider in Artifact Keeper with that issuer and a dedicated
audience, for example `artifact-keeper`, and identity mappings for the
ServiceAccounts that may pull. With Artifact Keeper 1.10 this is a `generic`
provider with one mapping per ServiceAccount. Follow
[Artifact Keeper 1.10: `generic` provider](../README.md#artifact-keeper-110-generic-provider),
steps 2 to 4. The issuer is public here, so its step 1 needs nothing.

### 2. Cluster: let nodes request tokens for the audience

Set `resources` in [`examples/rbac.yaml`](../examples/rbac.yaml) to your audience
and apply it:

```sh
kubectl apply -f examples/rbac.yaml
```

### 3. Configure the installer

In a copy of [`examples/installer/daemonset.yaml`](../examples/installer/daemonset.yaml):

- `provider.yaml` in the ConfigMap: `matchImages`, `--url`, and
  `serviceAccountTokenAudience`, which must equal the provider's audience in
  Artifact Keeper. Keep `name: ak-kubelet-provider`. A `--ca-file` is read by
  the kubelet, so it must be a path on the host, not in the pod.
- `BINARY_BASE_URL`: the release download URL, or an internal mirror with the
  same file names. The nodes must be able to reach it. On private GKE clusters
  or AKS clusters with restricted egress, that usually means a mirror.
- `SHA256_AMD64` and `SHA256_ARM64`: the values from the release's `.sha256`
  files, **after** verifying their signatures as the README shows.
- `image`: pin `mikefarah/yq` by digest, or use a copy in your own registry. The
  image must **not** come from Artifact Keeper through this plugin: the plugin
  is not installed until the installer has run.
- `https_proxy`, if nodes reach the internet through a proxy.

### 4. Apply and watch the rollout

```sh
kubectl apply -f daemonset.yaml
kubectl -n kube-system rollout status ds/ak-kubelet-provider-installer
kubectl -n kube-system logs -l app.kubernetes.io/name=ak-kubelet-provider-installer --prefix
```

A node that got the plugin logs:

```
ak-installer: installed /home/kubernetes/bin/ak-kubelet-provider (<sha256>)
ak-installer: updated /etc/srv/kubernetes/cri_auth_config.yaml (install)
ak-installer: restarting kubelet
ak-installer: kubelet healthy
```

Later passes log nothing unless they change something.

### 5. Verify a pull

Start a pod whose image is in Artifact Keeper, with a ServiceAccount that an
identity mapping allows and **no** `imagePullSecrets`. If it fails, see the
README's troubleshooting table. On GKE and AKS, the kubelet log is in
`journalctl -u kubelet` on the node.

## Operating it

- **A pod that is not Ready** means its last pass failed. Its log says why:
  download or checksum failure, a config the kubelet refused, or no credential
  provider flags on the kubelet.
- **Changing the provider entry or the pins:** edit the ConfigMap or the env, and
  bump the `ak-kubelet-provider/revision` annotation to roll it out node by node.
  Without the bump, a change to `provider.yaml` still reaches each node within a
  few minutes, but on all nodes at once. A change to `install.sh` takes effect
  only when the pods restart.
- **A config the kubelet refused** stays blocked on that node until the desired
  config changes. Fix `provider.yaml` and roll out again.
- **Uninstall:** set `MODE=uninstall` and wait for the rollout. Every node
  removes the entry and the binary, and restarts the kubelet. Then delete the
  DaemonSet and the ConfigMap. Deleting the DaemonSet alone leaves the plugin
  configured on existing nodes until they are replaced.

## Security

The installer pod is root on every node: it is privileged, shares the host PID
and network namespaces, and mounts the host's `/`. It runs the script from its
ConfigMap. Anyone who can change that ConfigMap or the DaemonSet in
`kube-system` can run code as root on every node, so restrict write access to
them accordingly. The pod does not mount a ServiceAccount token.
