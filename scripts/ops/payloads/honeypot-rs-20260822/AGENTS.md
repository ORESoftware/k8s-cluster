# Agent instructions

- Never place production credentials, source IPs, raw request bodies, authorization values, cookies, or query strings in this repository or its logs.
- Preserve the low-interaction boundary. Do not add shells, command execution, packet capture, malware handling, or unrestricted upload paths.
- Keep all automated response actions temporary and reversible.
- Do not route volumetric DDoS traffic to the origin.
- Application Kubernetes manifests must remain namespace-scoped; platform tenancy and Argo registration belong in `ORESoftware/k8s-cluster`.
- Use feature branches and reviewed pull requests. Never push implementation directly to `main`.
