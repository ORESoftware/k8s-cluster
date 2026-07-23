# Generated desired state

`gitops/ec2` is generated only by the approved monorepo deployment workflow.
Do not hand-edit rendered manifests or replace image digests with mutable tags.

The cluster bootstraps `apps/scintilla-run-infra/k8s/argocd/root-application.yaml`.
That root application synchronizes the generated AppProject/ApplicationSet,
which then synchronizes the control plane and runner child applications.
