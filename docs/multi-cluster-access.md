# Multi-cluster access runbook — AWS EC2 & Hetzner

*2026-07-25. How to reach the two Kubernetes runtimes for ops/testing, and the
credential surfaces involved. No secrets are recorded here — only where they
live and which handle to use.*

The fleet runs on **two** clusters. Know which one you are targeting: they have
independent node sets, images, and health.

## AWS EC2 runtime

- **Node:** single `dd-remote-k8s-1` (`i-0cc2461a55d491af6`, r7i.4xlarge,
  public `98.90.186.114`), account `710156900967`, region `us-east-1`.
- **AWS creds:** `~/.aws` profiles. `default` / `my-cli-user` are valid
  (`aws sts get-caller-identity` → `arn:…:user/my-cli-user`). `dd-cluster`,
  `prod`, `dd-codex` returned invalid/empty at time of writing — prefer
  `default`.
- **Shell access:** direct SSH is **key-denied**. The node is **SSM-managed**
  (`aws ssm describe-instance-information` → `Online`), so drive it with
  `aws ssm send-command --document-name AWS-RunShellScript`. For multi-line
  scripts, base64-encode and run `echo <b64> | base64 -d | bash` to sidestep the
  CLI's JSON-parameter quoting; poll `aws ssm get-command-invocation` until the
  status is terminal.
- **kubectl on the node:** `export KUBECONFIG=/home/ec2-user/.kube/config`
  (kubectl at `/usr/bin/kubectl`).
- **Gateway:** `https://98.90.186.114` fronts services under path prefixes
  (`/browser-test/*`, `/cluster-mcp`, `/mcp`, …) but gates them behind the
  operator `dd` header, and uses a private CA (pin via `mcp/fetch-gateway-ca.sh`;
  `curl -k` for a throwaway check). It does not expose authenticated in-cluster
  endpoints like `/run` or `/builds` internals.

## Hetzner runtime

- **Nodes:** `dd-k8s-fsn1` (`10.20.0.2`), `dd-k8s-nbg1` (`.3`), `dd-k8s-hel1`
  (`.4`) control-plane; `dd-k8s-wrk1` (`.7`), `dd-k8s-wrk2` (`.8`) workers.
  Kubernetes v1.31.14. Public bastion `hetzner-k8s-bastion` /
  `dd-k8s-fsn1-public` = `167.233.100.88` (root).
- **SSH:** `~/.ssh/config` already defines the hosts; key `~/.ssh/id_hetzner`.
  `ssh hetzner-k8s-bastion 'kubectl get nodes'` works directly (kubeconfig is set
  up for root on the control-plane node). Internal nodes (`10.20.0.x`) are
  reachable through the bastion.

## SSH key inventory (handles only)

`~/.ssh` holds, among others: `id_ed25519` (general), `id_hetzner` (Hetzner),
`id_argocd_dd` (Argo CD), plus GitHub identity aliases
(`github.com-oresoftware`, `-the1mills`, `-alex-sevendwarves`). Never print key
material; reference by path.

## Safety notes

- Prefer read-only probes first (`get`, `describe`, `logs`). Mutations on these
  clusters are live-traffic-affecting.
- To use a service's auth secret (e.g. `SERVER_AUTH_SECRET`), `kubectl exec` into
  the owning pod and read it from the pod's own env inline — do not extract it to
  a local variable or command line where it could be logged.
- Deleting `Evicted`/`Failed` pods is safe hygiene; scaling, `rollout restart`,
  and manifest edits are not — treat them as changes and confirm intent.
- The two clusters drift: a service healthy on AWS can be crash-looping on
  Hetzner (see [`browser-e2e-testing.md`](browser-e2e-testing.md) for a concrete
  case), so always confirm which cluster a report is about.
