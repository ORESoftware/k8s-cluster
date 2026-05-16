# Minikube EC2 Mirror

This overlay mirrors the EC2 runtime topology on a laptop Minikube cluster without editing the EC2
manifests. It imports the production ArgoCD bases, then patches only local-only differences:

- repo `hostPath` mounts point at `/workspace/k8s-cluster`
- gateway TLS is generated inside the pod instead of using the EC2 Kubernetes TLS secret
- EC2-only `containerd`/`nerdctl` mounts are disabled for the reaper, REST API lambda image builder,
  and Gleam lambda container prewarm path
- KEDA's `ScaledObject` is omitted so a plain Minikube cluster can apply the overlay before KEDA is
  installed; the queue-consumer Deployment still runs with the EC2 settings
- local placeholder Secrets provide the same Kubernetes Secret names used on EC2

An exact EC2 clone is not possible on stock Minikube because the live box depends on EC2 host paths,
AWS-backed External Secrets, local containerd tooling, a public IP/TLS renewal path, and optional
KEDA CRDs. This overlay is the closest practical mirror: same services, names, namespaces, ports,
gateway routing, NATS, and observability stack, with EC2-only node integrations made local-safe.

## Quickstart

From the repo root:

```bash
minikube start --driver=docker --cpus=6 --memory=12288 --disk-size=60g
```

Keep this running in a separate terminal so pods can see the checked-out source tree:

```bash
minikube mount "$PWD:/workspace/k8s-cluster"
```

Build the two local images the EC2 manifests expect as node-local images:

```bash
minikube image build -t dd-remote-web-home:dev remote/web-home-rs

eval "$(minikube docker-env)"
DOCKER_BUILDKIT=1 docker build \
  --build-arg DD_REPO_URL=git@github.com:ORESoftware/k8s-cluster.git \
  --build-arg DD_REPO_REF=dev \
  --secret id=github_deploy_key,src="${GH_DEPLOY_KEY_PATH:-$HOME/.ssh/id_ed25519}" \
  -t dd-dev-server:dev \
  remote/dev-server
```

`local-secrets.yaml` intentionally contains only placeholder values. Do not commit real credentials
there. When you need real agent runs, GitHub PRs, or Postgres-backed lambda/task data, copy that
file outside the repo, edit the copy, and apply it after the mirror overlay:

```bash
cp remote/minikube/ec2-mirror/local-secrets.yaml /tmp/dd-local-secrets.yaml
$EDITOR /tmp/dd-local-secrets.yaml
kubectl apply -f /tmp/dd-local-secrets.yaml
```

Restart any pods that already read those secrets after applying the private copy.

Apply the mirror:

```bash
kubectl apply -k remote/minikube/ec2-mirror
```

Open the gateway through port-forwarding:

```bash
kubectl -n default port-forward deployment/dd-remote-gateway 8080:80 8443:443
```

Then visit `http://127.0.0.1:8080/home`. HTTPS is available at
`https://127.0.0.1:8443/home` with a short-lived self-signed certificate generated in the gateway
pod.
