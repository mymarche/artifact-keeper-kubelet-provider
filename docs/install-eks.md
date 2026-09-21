# Install on Amazon EKS

This guide adds `ak-kubelet-provider` to EKS nodes that run the EKS-optimized
**Amazon Linux 2023** AMI. These nodes are configured by `nodeadm` from the
instance user data. The plugin is added **next to** `ecr-credential-provider`,
which the node still needs for ECR images, including the `aws-node`,
`kube-proxy` and other add-on images.

> **Verification status.** The mechanism was checked on its parts: the `jq` merge
> ran against the `config.json` that `nodeadm` generates for kubelet 1.32+, and
> on a Kubernetes 1.34.3 kubelet the last `--image-credential-provider-config`
> flag won. It has **not yet been run on a real EKS cluster.** Report the
> result, good or bad, in an issue.

## Where it applies

| Node type                                        | Supported |
|--------------------------------------------------|-----------|
| AL2023 managed node group with a launch template | yes       |
| AL2023 self-managed nodes                        | yes       |
| Karpenter with an AL2023 `EC2NodeClass`          | yes       |
| Bottlerocket                                     | no: it runs only the credential providers it ships |
| EKS Auto Mode, Fargate                           | no: there is no access to the node's kubelet config |
| Amazon Linux 2                                   | no: there are no EKS AL2 AMIs for Kubernetes 1.33+ |

The cluster and its nodes must run Kubernetes **1.34 or later**.

## How it works

On AL2023, `nodeadm` configures the kubelet in two systemd units, and cloud-init
runs the user-data shell scripts between them:

1. `nodeadm-config.service` runs before cloud-init. It writes
   `/etc/eks/image-credential-provider/config.json`, which lists only
   `ecr-credential-provider`, and it rewrites that file on every boot.
2. cloud-init runs the shell part of
   [`examples/eks/user-data.txt`](../examples/eks/user-data.txt). The script
   installs the binary in `/etc/eks/image-credential-provider/`, the bin dir
   that `nodeadm` already passes to the kubelet. It then writes
   `ak-config.json`: nodeadm's `config.json` with our provider entry appended.
3. `nodeadm-run.service` runs after cloud-init and starts the kubelet. The
   `NodeConfig` part of the user data adds
   `--image-credential-provider-config=.../ak-config.json`. `nodeadm` puts
   user-supplied `spec.kubelet.flags` **after** its own flags, and the kubelet
   uses the last value of a repeated flag, so the kubelet reads
   `ak-config.json`.

Because the ECR entry is copied from `nodeadm` at boot, it stays current when
the AMI changes the list of ECR hosts.

## Steps

### 1. Artifact Keeper: a provider for the cluster

EKS publishes the cluster's OIDC discovery document and keys, so Artifact Keeper
can fetch them directly. The issuer is:

```sh
aws eks describe-cluster --name <cluster> --query cluster.identity.oidc.issuer --output text
```

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

EKS nodes authenticate as members of `system:nodes`, the group that manifest
binds.

### 3. Prepare the user data

Copy [`examples/eks/user-data.txt`](../examples/eks/user-data.txt) and set:

- `BASE_URL`: the release download URL, or an internal mirror with the same
  file names. Nodes in private subnets without NAT cannot reach GitHub.
- `SHA256_AMD64` and `SHA256_ARM64`: the values from the release's `.sha256`
  files, **after** verifying their signatures as the README shows. The script
  refuses a binary that does not match.
- the provider entry: `matchImages`, `--url`, and `serviceAccountTokenAudience`,
  which must equal the provider's audience in Artifact Keeper. For a private
  CA, add `--ca-file` with a host path and put the CA on the node in the same
  script.

If you build your own AMI, install the binary into
`/etc/eks/image-credential-provider/` at build time and delete step 1 of the
script. Keep steps 2 and 3: they must run at boot, because `nodeadm` writes
`config.json` at boot.

### 4. Attach the user data to the nodes

**Managed node group with the EKS-optimized AMI** (no `image_id` in the launch
template). EKS adds its own `NodeConfig` with the cluster details and merges
it with this user data. Terraform:

```hcl
resource "aws_launch_template" "nodes" {
  name_prefix = "${var.cluster_name}-nodes-"
  user_data   = filebase64("${path.module}/user-data.txt")

  # A launch template replaces the node group's disk_size.
  block_device_mappings {
    device_name = "/dev/xvda"
    ebs {
      volume_size = 50
      volume_type = "gp3"
    }
  }
}

resource "aws_eks_node_group" "this" {
  # ...
  ami_type = "AL2023_x86_64_STANDARD"   # or AL2023_ARM_64_STANDARD
  launch_template {
    id      = aws_launch_template.nodes.id
    version = aws_launch_template.nodes.latest_version
  }
}
```

**Custom AMI or self-managed nodes.** EKS does not add the cluster details, so
add a `NodeConfig` part with `spec.cluster` (`name`, `apiServerEndpoint`,
`certificateAuthority`, `cidr`) to the same MIME document.

**Karpenter.** Put the same MIME document in the `EC2NodeClass`'s
`spec.userData` with `amiFamily: AL2023`. Karpenter merges its own `NodeConfig`.

Existing nodes do not pick up user-data changes. A new launch template version
makes a managed node group replace its nodes, one at a time with
`max_unavailable = 1`. For Karpenter, the drift controller replaces them.

### 5. Verify on a node

From an SSM session on a new node:

```sh
sudo tr '\0' '\n' < /proc/$(pidof kubelet)/cmdline | grep image-credential-provider
# --image-credential-provider-bin-dir=/etc/eks/image-credential-provider
# --image-credential-provider-config=/etc/eks/image-credential-provider/config.json
# --image-credential-provider-config=/etc/eks/image-credential-provider/ak-config.json   <- last one wins

sudo jq -r '.providers[].name' /etc/eks/image-credential-provider/ak-config.json
# ecr-credential-provider
# ak-kubelet-provider
```

Then start a pod whose image is in Artifact Keeper, with a ServiceAccount that
an identity mapping allows and **no** `imagePullSecrets`. If the pull fails,
see the README's troubleshooting table.

## When something goes wrong

- **The node never joins the cluster.** If the script fails (download, checksum,
  `jq`), `ak-config.json` is never written. The kubelet then fails to start
  because its config file is missing. That is deliberate: the node never runs
  without the plugin you asked for. The reason is in
  `/var/log/cloud-init-output.log`. The release download is the usual cause,
  so use a mirror or bake the binary into the AMI.
- **ECR pulls fail on the new nodes.** Check that `ak-config.json` still lists
  `ecr-credential-provider`. The script copies it from nodeadm's
  `config.json`, so a script edited to write the file from scratch loses it.
- **The plugin is never called.** The kubelet uses `config.json`, not
  `ak-config.json`. Check that the `--image-credential-provider-config` flag
  from the `NodeConfig` part is the last one on the kubelet's command line.

## Upgrading and removing

Both work by replacing nodes. For an upgrade, change `BASE_URL` and the pins in
a new launch template version. For removal, roll out a version without this
user data.
