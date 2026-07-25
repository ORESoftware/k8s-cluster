# Patch 1 — run the ECR image, not a runtime `cargo run` of `origin/dev`

**Finding (CRITICAL, audit F13).** `remote/argocd/dd-next-runtime/dd-build-server.deployment.yaml`
runs `image: docker.io/library/rust:1.90-bookworm` with an inline
`git clone --depth 1 --branch dev … && cargo run --release` at every pod start.
The multi-stage `Dockerfile` is never used. Anyone who can push `dev` (or
compromise `GH_PAT`) gets code execution in a pod holding the containerd socket
= node root, with no image build, digest, scan, or review gate. Pods started an
hour apart run different code; clone failure falls back to a developer working
tree (`/home/ec2-user/codes/dd/dd-next-1`) on the node.

The image already exists: the webhook rule in
`dd-build-server.configmap.yaml` builds
`710156900967.dkr.ecr.us-east-1.amazonaws.com/dd-build-server:dev-{shortSha}` on
every push to `dev`. The deployment simply doesn't consume it.

## Change

Replace the container's `image` + `command`/`args` bootstrap with the ECR image,
and delete the now-unused git-clone plumbing.

```yaml
# dd-build-server.deployment.yaml — container spec
      containers:
        - name: build-server
          # Pin by digest in CI (kustomize `images:` newTag/digest, or Argo Image
          # Updater). The tag shown is the shape the webhook rule already pushes;
          # a digest (…@sha256:…) is required to kill drift completely.
          image: 710156900967.dkr.ecr.us-east-1.amazonaws.com/dd-build-server:dev-<shortSha>
          imagePullPolicy: IfNotPresent
          # DELETE the `command:`/`args:` block entirely — the image's ENTRYPOINT
          # is the compiled binary. No bash, no cargo, no clone.
          securityContext:
            allowPrivilegeEscalation: false
            capabilities: { drop: [ALL] }
            seccompProfile: { type: RuntimeDefault }
            # Now possible because we no longer need to write CARGO_TARGET_DIR etc.
            # (Keep root only if the containerd socket group requires it — see
            # patch 4; with rootless BuildKit this becomes runAsNonRoot: true.)
            readOnlyRootFilesystem: true
```

Also delete these, which only existed to feed the runtime clone/compile:

- env `BUILD_SERVER_GIT_URL`, `BUILD_SERVER_GIT_REF`
- env `CARGO_HOME`, `CARGO_TARGET_DIR` (build-time only)
- the `repo` hostPath volume (`/home/ec2-user/codes/dd/dd-next-1`) and its mount
- the `GH_PAT`-in-argv `git -c http.extraheader=` path (audit F12) — it has no
  remaining caller once the clone is gone

`HOME=/tmp` + the `tmp` emptyDir stay (job scratch). The Dockerfile should also
gain `USER 65532:65532` and pin its base images by digest.

## Wiring: how CI supplies the tag/digest

The webhook build already produces `dev-{shortSha}`. Options, cheapest first:

1. **kustomize `images:`** in `remote/argocd/dd-next-runtime/kustomization.yaml`:
   ```yaml
   images:
     - name: 710156900967.dkr.ecr.us-east-1.amazonaws.com/dd-build-server
       newTag: dev-<shortSha>   # CI patches this on successful build
   ```
2. **Argo CD Image Updater** annotation tracking `dev-*` by newest-build, writing
   back the digest.

Either way the running version becomes an auditable, ArgoCD-visible artifact.

## Rollback

Revert the deployment block; the previous `rust:1.90-bookworm` + clone/compile
is fully self-contained. Because the ECR image is immutable, a bad build rolls
back by pointing the tag/digest at the prior one — no code state to unwind.

## Depends on

- The ECR image being buildable **without** the runtime clone — it is; the
  webhook rule builds `contextDir: remote/deployments/build-server-rs` with the
  committed `Dockerfile`.
- `BUILD_SERVER_PUSH_ENABLED=true` + working ECR login (already set).
