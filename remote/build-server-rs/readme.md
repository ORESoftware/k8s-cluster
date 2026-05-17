# `dd-build-server`

Small Rust CI/CD control surface for the EC2 Kubernetes runtime.

It exposes authenticated JSON endpoints for:

- `POST /builds` to clone a repo, build an image with `nerdctl`, and optionally deploy a manifest
  or kustomize overlay with `kubectl`.
- `GET /builds` and `GET /builds/<jobId>` to inspect in-memory build state.
- `GET /builds/<jobId>/logs` to read the capped build log from disk.
- `GET /healthz` and `GET /metrics` for probes and Prometheus.

The server intentionally does not accept arbitrary shell commands. A submitted job must declare:

- `repoUrl`: `https://`, `ssh://`, or `git@` repo URL.
- `gitRef`: optional branch or tag, passed to `git clone --branch`.
- `image`: image tag to build.
- `contextDir` and `dockerfile`: relative paths inside the cloned repo.
- `deploy.kind`: `kustomize`, `manifest`, or `none`.
- `deploy.path`: relative path inside the cloned repo.
- `deploy.namespace`: namespace allowlisted by `BUILD_SERVER_ALLOWED_NAMESPACES`.

Example:

```json
{
  "repoUrl": "https://github.com/example/app.git",
  "gitRef": "main",
  "image": "docker.io/library/example-app:dev",
  "contextDir": ".",
  "dockerfile": "Dockerfile",
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

`SERVER_AUTH_SECRET` must come from `dd-agent-secrets`. Pushes are disabled unless
`BUILD_SERVER_PUSH_ENABLED=true`, and deploys are limited to namespaces listed in
`BUILD_SERVER_ALLOWED_NAMESPACES`.
