# Deploy paths

Deployment is per-app; the monorepo's job is to pin a coherent set of
submodule SHAs and run cross-repo checks (see `.github/workflows/ci.yml`).

| App | How it deploys |
|-----|----------------|
| `apps/sonus-auris-backend.rs` | Gitops via [`~/codes/ores/k8s-cluster`](https://github.com/ORESoftware) — the cluster repo tracks it as submodule `remote/deployments/dd-sound-recorder-rs`; bump that pointer and push the cluster repo. |
| `apps/sonus-auris-web-server.rs` | Gitops via `k8s-cluster` (same pattern; add its deployment submodule when the manifest lands). |
| `apps/sonus-auris-api-server.rs` | Gitops via `k8s-cluster` (same pattern; JSON API + billing webhooks). |
| `apps/sonusauris-app-proxy` | GitHub Actions in its own repo: `deploy.yml` runs `wrangler deploy` on push to `main` (needs `CLOUDFLARE_API_TOKEN` repo secret). |
| `apps/sonus-auris-site.web` | GitHub Pages workflow in its own repo; served through the Cloudflare proxy at `sonusauris.app`. |
| `apps/sonus-auris-ui.dart` | Store releases (Play / App Store) via `scripts/release/`. |
| `apps/sonus-auris-web-desktop.dart` | Web build to static hosting (own workflow); desktop bundles per-OS. |
| `apps/sonus-auris-interfaces` | Not deployed — schema/codegen source of truth. Supabase migrations under `supabase/migrations/` are applied manually after review (see its README). |

The cluster repo (`~/codes/ores/k8s-cluster`) also tracks **this monorepo** as
submodule `remote/submodules/sonus-auris-monorepo`, so a cluster pin records
the exact all-app state that was live when a deploy happened.
