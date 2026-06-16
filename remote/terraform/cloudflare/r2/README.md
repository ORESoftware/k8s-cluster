# Cloudflare R2 — general-purpose object storage for `ores-k8s-cluster`

Provisions a **logical namespace of R2 buckets** for the cluster (Cloudflare account
`alexander.d.mills@gmail.com`), list-driven so adding a bucket is a one-line change,
plus **edge caching** (parametrized — off until you attach a Cloudflare zone).

R2 has no namespace primitive (buckets are flat per account), so the namespace is the
**`ores-k8s-` bucket prefix** (`var.bucket_prefix`). The matching cluster-side namespace
is the Kubernetes `ores-k8s-storage` namespace under `remote/k8s/storage/`.

## What it creates

| Resource | Purpose |
|---|---|
| `cloudflare_r2_bucket` (for_each) | One bucket per entry in `var.buckets`, named `ores-k8s-<key>`. |
| `cloudflare_r2_custom_domain` (for_each) | Per-bucket custom domain — **only when `edge_domain`/`edge_zone_id` are set** and the bucket has `edge_cache = true`. This is what puts Cloudflare's CDN in front of R2. |
| `cloudflare_ruleset` (cache phase) | One zone cache rule that edge-caches the custom-domain hostnames at `edge_cache_ttl`. |
| `cloudflare_account_token` | Account-scoped R2 read/write token; its id + sha256(value) are the S3 access-key pair. |

Default buckets: `artifacts`, `media`, `public-assets`, `backups` (Infrequent Access),
`cache`, `general`. `media` and `public-assets` are flagged `edge_cache = true`.

## Why the raw S3 endpoint is not cached

SDK traffic to `https://<account-id>.r2.cloudflarestorage.com` bypasses the CDN. Edge
caching only happens on a **custom domain** attached to the bucket (or the rate-limited
`r2.dev` dev URL). That's why `edge_domain` exists: set it once you add a domain to the
account and the custom-domain + cache-rule resources light up with no change to buckets.

## Apply

```bash
cd remote/terraform/cloudflare/r2
cp terraform.tfvars.example terraform.tfvars   # fill in account_id

# Account API token with: R2 Admin Read & Write + Workers R2 Storage Edit
# (+ Cache Rules Edit + DNS Edit on the zone, once you enable edge_domain).
export CLOUDFLARE_API_TOKEN=...

terraform init
terraform plan
terraform apply
```

### Wire the credentials into the cluster

```bash
kubectl apply -f ../../../k8s/storage/00-namespace.yaml
# Fill R2_ENDPOINT (account id) in the configmap, then:
kubectl apply -f ../../../k8s/storage/10-r2-config.configmap.yaml

cp ../../../k8s/storage/20-r2-credentials.secret.template.yaml \
   ../../../k8s/storage/20-r2-credentials.secret.yaml   # gitignored
# Paste the two values:
terraform output -raw r2_access_key_id
terraform output -raw r2_secret_access_key
kubectl apply -f ../../../k8s/storage/20-r2-credentials.secret.yaml
```

Workloads consume both:

```yaml
envFrom:
  - configMapRef: { name: r2-config }
  - secretRef:    { name: r2-credentials }
```

### Enable edge caching later

1. Add a domain to the Cloudflare account, grab its zone id.
2. Set `edge_domain`, `edge_zone_id` (and optionally `edge_subdomain`, `edge_cache_ttl`) in `terraform.tfvars`.
3. `terraform apply` → custom domains like `media.cdn.<edge_domain>` + the cache rule appear.
4. Copy `terraform output edge_domains` into the `R2_PUBLIC_BASE_URL_*` keys in the configmap.

## Notes

- The Cloudflare provider v5 schema differs from v4. Run `terraform validate` against the
  pinned provider after `init`; if an attribute name drifts, adjust `main.tf` (the resource
  set is correct, only field spellings churn between minor versions).
- State holds the R2 secret — use a remote backend / keep `*.tfstate` gitignored (already is).
