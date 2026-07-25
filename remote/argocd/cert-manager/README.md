# cert-manager — bringing TLS under GitOps

**Status: scaffolding. Nothing here is wired into any cluster apply yet**, matching the
house convention for boundary changes (same posture as [`../projects/`](../projects/) and
[`../gateway-edge/`](../gateway-edge/)). Applying it is a deliberate, eyes-on migration.

Background and the reasoning for doing this at all:
[`docs/crossplane-and-custom-operators.md`](../../../docs/crossplane-and-custom-operators.md) §3.2.

## The problem this fixes

TLS is currently handled two different ways, and neither is described by Git:

| | Hetzner | AWS |
|---|---|---|
| Edge | ingress-nginx DaemonSet, hostNetwork | none — gateway pod binds hostPort 80/443 |
| Cert issuance | cert-manager + `letsencrypt-prod` (HTTP-01) | certbot on the EC2 host |
| Installed by | `remote/hetzner/ingress-tls.sh` (imperative `kubectl apply`) | `remote/ec2/renew-letsencrypt-gateway-cert.sh` + systemd timer |
| Secret delivery | cert-manager writes `gateway-public-tls` | `kubectl create secret tls` from the host, then `rollout restart` |
| In Git as desired state? | **No** | **No** |

So the component that secures every public hostname is the one thing ArgoCD does not
reconcile. Concretely, that has already cost us:

- The AWS renewal hook broke once because the cert lineage name was hard-coded to a stale
  EC2 IP (`54.91.17.58`); it now reads `$RENEWED_LINEAGE` to survive IP changes. A cert
  bound to an IP that can change is a recurring failure, not a one-time bug.
- A failed systemd-timer renewal is silent until the certificate actually expires.
  cert-manager retries across the whole 30-day `renewBefore` window and reports
  `Certificate` status the cluster can alert on.
- Rebuilding either cluster means re-running a script by hand and hoping it matches.

## Phase 1 — adopt what is already running (Hetzner, GCP)

[`clusterissuers.yaml`](clusterissuers.yaml) is an exact mirror of the issuers
`ingress-tls.sh` applies, so this phase changes no live behavior. It only moves ownership.

1. Apply the Applications: `kubectl apply -f ../apps/cert-manager.application.yaml`
2. Expect **field-manager conflicts on the first sync**. The live objects were created by
   `kubectl apply --server-side` (field manager `kubectl`); ArgoCD applies as its own
   manager. `ServerSideApply=true` is set for this reason — watch the first sync rather
   than assuming it. If a conflict sticks, resolve it on the specific object; do not
   delete and recreate cert-manager to get a clean slate.
3. Confirm `kubectl get clusterissuer` shows all three `Ready` and that no `Certificate`
   went `False`. The gateway cert (`gateway-public-tls`) is the one that matters.
4. Only then, delete the cert-manager and ClusterIssuer stanzas from
   `remote/hetzner/ingress-tls.sh` so the script stops competing for ownership. Until that
   edit lands, treat the script as "do not run".

Note the split into two Applications: the chart owns the CRDs, the issuers are CRs of
those CRDs. In one Application they race on a fresh cluster and the first sync fails.

## Phase 2 — retire the certbot glue (AWS)

This is the phase with real prerequisites, all in [`dns01-cloudflare/`](dns01-cloudflare/).

**Why a different solver.** HTTP-01 cannot work on AWS. cert-manager's HTTP-01 solver
needs to answer on port 80 at the challenged name, and the gateway pod already owns
hostPort 80 (and 308-redirects it to :443). There is no ingress controller to hand the
challenge to. DNS-01 proves control by writing a TXT record through the Cloudflare API
instead, so nothing needs to be reachable on :80 and no ingress controller is required.

**The blocking prerequisite.** The AWS gateway currently serves a Let's Encrypt
*IP address* certificate — the certbot lineage is named after the EC2 public IP. ACME does
not allow DNS-01 for IP identifiers. **The gateway must get a real DNS name under
`fiducia.cloud` before this phase is possible.** That is not a workaround; it is the
durable fix for the stale-lineage breakage above.

Order of operations:

1. Create the Cloudflare A record (e.g. `aws.fiducia.cloud` → EC2 public IP) and set the
   real name in `dns01-cloudflare/gateway-certificate.yaml`.
2. Seed a Cloudflare API token scoped to **Zone:DNS:Edit on `fiducia.cloud` only** into AWS
   Secrets Manager at `dd/remote-dev/cloudflare`, key `CLOUDFLARE_DNS_API_TOKEN`. This is a
   new, narrower token — not the broader one Terraform uses for R2.
3. On the EC2 node, stop the competing writer **first**:
   `sudo systemctl disable --now dd-letsencrypt-renew.timer`. cert-manager and certbot
   both write the same `dd-remote-gateway-tls` secret; leaving both active means they
   overwrite each other.
4. Add `dns01-cloudflare` to an Application (or this dir's `kustomization.yaml`) and sync.
   Watch `kubectl describe certificate dd-remote-gateway-tls` through issuance.
5. Make the gateway reload on rotation. Reloader is already deployed cluster-wide
   (`../observability/reloader.deployment.yaml`, `ClusterRole`-scoped, running with
   `--auto-reload-all=false` so it acts only where annotated). Add to the gateway
   Deployment's pod template:
   `secret.reloader.stakater.com/reload: dd-remote-gateway-tls`.
   Without it the kubelet refreshes the mounted files but nginx keeps serving the old cert
   from memory. **This edit is intentionally not made here** — `dd-next-runtime/` is synced
   with `prune: true, selfHeal: true` on both live clusters, so a commit there takes effect
   immediately. Make it when you are running the migration, not before.
6. Once a renewal has been observed to succeed, delete
   `remote/ec2/renew-letsencrypt-gateway-cert.sh` and the
   `dd-letsencrypt-renew.{service,timer}` units.

## What this does not do

It does not install ingress-nginx on AWS, and it does not move the AWS gateway behind an
ingress controller. That would converge the two clouds' edges completely, and the
scaffolding for it already exists in [`../gateway-edge/`](../gateway-edge/) — but it is a
data-path change, whereas everything above is a control-plane change that leaves traffic
flowing exactly as it does today. Keep those two migrations apart.
