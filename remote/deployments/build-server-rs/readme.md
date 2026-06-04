# `dd-build-server`

Small Rust CI/CD control surface for the EC2 Kubernetes runtime.

It exposes authenticated JSON endpoints for:

- `POST /builds` to clone a repo, build an image with `nerdctl`, and optionally deploy a manifest
  or kustomize overlay with `kubectl`.
- `GET /builds` and `GET /builds/<jobId>` to inspect in-memory build state.
- `GET /builds/<jobId>/logs` to read the capped build log from disk.
- `GET /healthz` and `GET /metrics` for probes and Prometheus.

The server intentionally does not accept arbitrary shell commands. A submitted job is a
`build-server.v1` JSON document:

- `schemaVersion`: optional; when present it must be `build-server.v1`.
- `jobKind`: optional; `build-image` or `build-and-deploy`.
- `repoUrl`: `https://`, `ssh://`, or `git@` repo URL.
- `gitRef`: optional branch or tag, passed to `git clone --branch`.
- `image`: explicit image tag or digest to build. The deployment currently allowlists
  `710156900967.dkr.ecr.us-east-1.amazonaws.com/`.
- `contextDir` and `dockerfile`: relative paths inside the cloned repo.
- `buildArgs`: optional non-secret Docker build args. Keys containing `SECRET`, `PASSWORD`,
  `TOKEN`, `CREDENTIAL`, or `PRIVATE_KEY` are rejected and values are redacted from command logs.
- `push`: optional; when `true`, the image is pushed after a successful build.
- `deploy.kind`: `kustomize`, `manifest`, or `none`.
- `deploy.path`: relative path inside the cloned repo.
- `deploy.namespace`: namespace allowlisted by `BUILD_SERVER_ALLOWED_NAMESPACES`.

Example:

```json
{
  "schemaVersion": "build-server.v1",
  "jobKind": "build-and-deploy",
  "repoUrl": "https://github.com/example/app.git",
  "gitRef": "main",
  "image": "710156900967.dkr.ecr.us-east-1.amazonaws.com/example-app:dev",
  "contextDir": ".",
  "dockerfile": "Dockerfile",
  "push": true,
  "deploy": {
    "kind": "kustomize",
    "path": "k8s/overlays/ec2",
    "namespace": "default",
    "rollout": "deployment/example-app"
  }
}
```

The Argo runtime deployment runs this from the host-mounted checkout and mounts:

- the EC2 containerd socket
- `/usr/local/bin/nerdctl`
- `/usr/bin/kubectl`
- `/var/lib/dd-build-server`

`SERVER_AUTH_SECRET` must come from `dd-agent-secrets`. ECR push support is enabled by
`BUILD_SERVER_PUSH_ENABLED=true` and `BUILD_SERVER_ECR_LOGIN_ENABLED=true`. For ECR auth, provide
`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, and optional `AWS_SESSION_TOKEN` through
`dd-agent-secrets` or replace the env-based signer with an instance-role credential provider.
Deploys are limited to namespaces listed in `BUILD_SERVER_ALLOWED_NAMESPACES`.

## Security notes

This service is safe from arbitrary shell execution at the HTTP/API layer: callers can only request
the fixed sequence `git clone`, `nerdctl build`, optional ECR `nerdctl login` + `nerdctl push`, and
optional `kubectl apply` + `kubectl rollout status`.

It is not a fully untrusted code sandbox. A Dockerfile is code, and a Kubernetes Deployment
manifest is also code that can run pods. Today this build server should be used for trusted repos
and authenticated operators. For untrusted repos, run builds in a separate empty namespace with no
valuable secrets, replace the host containerd socket with rootless BuildKit or Kaniko, and add an
admission policy that blocks secret mounts, privileged pods, hostPath, host networking, and
service-account token automounting.

Current hardening in the Argo deployment:

- command execution uses direct argv, not `/bin/sh -c`
- child commands run with a stripped environment
- repo, image, namespace, and path allowlists
- ECR push only for allowlisted ECR image prefixes
- reduced Kubernetes RBAC: Deployments, Services, ConfigMaps, Ingresses, HPAs, and read-only Events
- no Secret, Pod, ServiceAccount, Job, DaemonSet, StatefulSet, or NetworkPolicy write permissions
- NetworkPolicy restricts ingress to the gateway/observability paths and egress to DNS, kube API,
  and public git/ECR endpoints
