# Stock EC2 Bootstrap

This directory is the no-custom-AMI path for bringing up the `remote/k8s`
single-node Kubernetes cluster on a plain Amazon Linux 2023 EC2 instance.
The Packer AMI in `remote/ami` is still the preferred production path because
it pre-bakes the slow tool installs; this path exists so a fresh EC2 box can be
scaffolded safely while the AMI is not available.

## Instance Requirements

Use at least:

- `x8i.2xlarge` or a comparable 128 GiB x86 host for the long-lived node
- 800 GiB gp3 root volume
- an instance profile with `AmazonEC2ContainerRegistryReadOnly` and
  `AmazonEBSCSIDriverPolicy`
- inbound security-group access for `6443` from Vercel/control-plane IPs
- inbound access for HTTP/HTTPS or your chosen ingress ports
- inbound UDP `51820` from trusted operator networks if deploying the WireGuard access layer
- inbound SSH `22` only from trusted operator IPs, or use AWS Systems Manager Session Manager instead

The bootstrap script can install prerequisites on a smaller instance for
smoke testing, but it refuses the cluster phase below about 2 GiB RAM unless
`--force-small-instance` or `DD_ALLOW_UNDERSIZED=1` is set. A `t3.micro` is
useful for validating clone and package setup, not for reliable remote-dev
thread pods or the long-lived `remote/dev-server` service.

## Operator Access

Prefer two separate access paths:

- Cluster access: deploy `remote/argocd/vpn`, connect through WireGuard, then use `dd-bastion` to
  fetch a read-only kubeconfig and deployment inventory. This avoids handing agents or browsers raw
  AWS credentials.
- Host shell access: use key-based SSH from a trusted IP or AWS Systems Manager Session Manager.
  Do not use a public MCP endpoint as a password-to-SSH or password-to-AWS bridge.

Local SSH config example:

```sshconfig
Host dd-ec2-runtime
  HostName 54.91.17.58
  User ec2-user
  IdentityFile ~/.ssh/dd-ec2-runtime.pem
  IdentitiesOnly yes
  ServerAliveInterval 30
```

## First-Time Setup

From your workstation:

```bash
ssh -i /path/to/main-key-pair.pem ec2-user@<public-ip>
mkdir -p ~/codes/dd
cd ~/codes/dd
git clone --branch dev git@github.com:ORESoftware/k8s-cluster.git dd-next-1
cd dd-next-1
```

The bootstrap script fast-forwards that checkout to `origin/dev` before it
installs Kubernetes or starts the cluster, so the host comes up on the same
branch as the repo you launched from.

Create the cluster secret file on the EC2 host:

```bash
cp remote/k8s/02-secrets.template.yaml remote/k8s/02-secrets.yaml
$EDITOR remote/k8s/02-secrets.yaml
```

Then run the bootstrap:

```bash
bash remote/ec2/bootstrap-amazon-linux-2023-k8s.sh --all
```

For a small instance where you only want the machine prepared but not yet
clustered:

```bash
bash remote/ec2/bootstrap-amazon-linux-2023-k8s.sh --prereqs-only
```

After resizing the instance and attaching the required IAM role, finish with:

```bash
bash remote/ec2/bootstrap-amazon-linux-2023-k8s.sh --cluster-only
```

## What It Installs

The script installs the minimal pieces needed by `remote/ami/bootstrap-cluster.sh`:

- base Amazon Linux packages such as `git`, `jq`, `curl`, `tar`, and networking tools
- kernel modules and sysctls required by Kubernetes networking
- `containerd`, `runc`, CNI plugins, `crictl`, and `nerdctl`
- `kubelet`, `kubeadm`, and `kubectl`
- `helm`, Cilium CLI, Hubble CLI, ArgoCD CLI, and the cached ArgoCD install manifest

The cluster phase delegates to `remote/ami/bootstrap-cluster.sh`, so both the
AMI path and stock-EC2 path converge on the same `remote/k8s` manifests.

## Let's Encrypt Gateway Certificate

The public gateway can serve a trusted Let's Encrypt certificate for the bare
EC2 IP. The runtime manifests expose `/.well-known/acme-challenge/` from the
host directory `/home/ec2-user/dd-acme-webroot`, so Certbot can use HTTP-01
webroot validation without stopping the gateway.

Use Certbot 5.4+ for IP-address certificates. On Amazon Linux 2023:

```bash
sudo dnf install -y python3.12 python3.12-pip
python3.12 -m venv /home/ec2-user/certbot-venv-312
/home/ec2-user/certbot-venv-312/bin/python -m pip install --upgrade pip setuptools wheel
/home/ec2-user/certbot-venv-312/bin/python -m pip install 'certbot>=5.4,<6'
```

Issue or renew the cert using the commands in
`remote/argocd/dd-next-runtime/readme.md`, then deploy the currently-issued
cert into Kubernetes:

```bash
bash remote/ec2/renew-letsencrypt-gateway-cert.sh deploy
```

Because Let's Encrypt IP-address certificates are short-lived, run renewal
several times per day from cron or a scheduler:

```bash
bash /home/ec2-user/codes/dd/dd-next-1/remote/ec2/renew-letsencrypt-gateway-cert.sh renew
```

The EC2 image should install the systemd timer in this directory:

```bash
sudo cp /home/ec2-user/codes/dd/dd-next-1/remote/ec2/dd-letsencrypt-renew.service /etc/systemd/system/dd-letsencrypt-renew.service
sudo cp /home/ec2-user/codes/dd/dd-next-1/remote/ec2/dd-letsencrypt-renew.timer /etc/systemd/system/dd-letsencrypt-renew.timer
sudo systemctl daemon-reload
sudo systemctl enable --now dd-letsencrypt-renew.timer
systemctl list-timers dd-letsencrypt-renew.timer
```

The timer runs `renew-letsencrypt-gateway-cert.sh renew` every six hours with a
randomized delay. Certbot skips if the certificate is not due; when renewal
happens, the deploy hook rewrites `dd-remote-gateway-tls` and restarts
`dd-remote-gateway`.
