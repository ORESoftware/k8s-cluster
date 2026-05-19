# `remote/argocd/headlamp`

GitOps-managed Headlamp deployment for the EC2 Kubernetes cluster.

## Access

The gateway serves Headlamp at:

```bash
https://54.91.17.58/headlamp/
```

The route is protected by the same gateway auth as `/telemetry/`. After the
Headlamp page loads, paste a Kubernetes bearer token. For a read-only operator
token:

```bash
kubectl -n headlamp create token headlamp-viewer
```

That token can inspect cluster, workload, pod, container, log, Argo CD, KEDA,
and External Secrets state. It does not grant write, exec, delete, or secret
read permissions.

## Deploy

Apply the Argo CD application:

```bash
kubectl apply -f remote/argocd/apps/headlamp.application.yaml
```

Or apply the kustomization directly:

```bash
kubectl apply -k remote/argocd/headlamp
kubectl -n headlamp rollout status deployment/headlamp
```

For a local check without the public gateway:

```bash
kubectl -n headlamp port-forward svc/headlamp 8081:80
open http://127.0.0.1:8081/headlamp/
```

