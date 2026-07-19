# Akrion GitOps release contract

`release.json` is the machine-readable record of the Akrion source revisions
currently promoted through this repository. The `akrion GitOps` GitHub Actions
workflow polls the three canonical Akrion repositories, selects the newest
successful `main` CI revision for each one, pins the matching gitlinks and
manifests, validates the desired state, and commits the promotion to `dev`.

The workflow never receives Kubernetes credentials and never runs `kubectl
apply`. Argo CD watches `k8s-cluster@dev` and is the only deployment actor.

The release covers four independently versioned inputs:

- `backend`: the private `dd-soccer-rs` HTTP/WebSocket server;
- `web`: the private Akrion Maud/HTMX portal;
- `soccer`: the public simulation and learning engine;
- `des`: the public optimization and discrete-event dependency.

Backend and web revisions are pinned both in `release.json` and in their git
submodule entries. Soccer and DES revisions are pinned in the runtime and
learning environment, so every pod/job records and checks out an immutable
commit instead of a moving branch name.

## Argo CD topology

- `dd-next-runtime` hosts the backend, portal, queue learner, and tournament on
  AWS and Hetzner.
- `dd-akrion-training` hosts the continuous learner and the legacy commit-watcher
  resources through cluster-specific overlays.
- AWS runs one continuous learner replica. Hetzner declares zero replicas because
  its 32 GiB nodes cannot safely run the 64 GiB-capped learner.
- The legacy in-cluster commit-watcher is declared at zero replicas in both
  clusters. GitHub Actions now advances desired state; the watcher must not make
  imperative rollout changes behind Argo CD's back.

The learner and watcher are adopted with partial Server-Side Apply manifests.
This is deliberate: the live Deployments contain tuned safeguards that are not
yet completely represented in the canonical source manifest. Argo owns only the
replica policy, immutable source pins, and rollout annotations; existing field
managers retain every other training parameter and runtime field.

The cluster application lists are bootstrap desired state rather than an
app-of-apps resource. On first installation, apply only the
`dd-akrion-training` Application CR to each cluster, then transfer the partial
Deployment fields once with server-side `--force-conflicts`. After that one-time
ownership transfer, Argo performs all reconciliation and no imperative workload
deployment is needed.

## Promotion

The promotion workflow runs every ten minutes, can receive an `akrion-release`
repository dispatch when a valid cross-repository token is configured, and can
be started manually:

```bash
gh workflow run akrion-gitops.yml \
  --repo ORESoftware/k8s-cluster \
  --ref dev
```

`REMOTE_DEV_GH_PAT` is the existing encrypted `k8s-cluster` Actions secret used
to read successful workflow metadata from the two private Akrion repositories
and push the resulting GitOps commit. It is not exposed to workloads or to Argo
CD. A future fine-grained cross-repository token can be used by source-side
notifiers for lower latency, but it is not required for automatic promotion.

Promotion stops when CI is red, a revision is malformed, a manifest no longer
matches the release contract, or either Kustomize overlay fails to render. A
later scheduled run retries safely after the underlying issue is fixed.
