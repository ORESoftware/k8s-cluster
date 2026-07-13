# dd-next-1-js — standalone Next.js app on the cluster

Runs the **same `dd-next-1` repo that ships to Vercel** as a long-lived standalone server in the
k8s cluster. **No fork / no branch** — the deploy target is env-gated:

- `next.config.ts`: `output: process.env.DD_DEPLOY_TARGET === 'standalone' ? 'standalone' : undefined`
  → Vercel builds unchanged (default output); the cluster build sets `DD_DEPLOY_TARGET=standalone`
  and gets `.next/standalone/server.js`.
- **Standalone-only deps (NATS) load via async `import()`** inside server routines
  (e.g. `src/lib/server/nats/nats-routines.ts`), so the Vercel bundle never pulls NATS in; the
  standalone runtime lazy-loads it and connects to `NATS_URL`
  (`nats://dd-nats.messaging.svc.cluster.local:4222`).

## Deploy

Manifests live in `remote/argocd/dd-next-runtime/dd-next-1-js.{deployment,service}.yaml` (registered
in that kustomization). The deployment clones the private `dd-next-1` repo with `GH_PAT` (from
`dd-agent-secrets`), runs `DD_DEPLOY_TARGET=standalone pnpm run build`, assembles the standalone
output, and runs `node server.js` on port 3000.

### Required before it will run

1. **App env secret** (NeonDB, Clerk, Supabase, OAuth, API keys, …) — create from the prod env file
   (never committed):
   ```bash
   kubectl create secret generic dd-next-1-js-env --from-env-file=env/.prod.env -n default
   ```
   The deployment loads it via `envFrom` (marked `optional` so the pod schedules without it, but the
   app needs it to function).
2. **Build resources/time**: the build needs ~14 GB old-space (`NODE_OPTIONS=--max-old-space-size=14336`)
   and several minutes; the deployment requests up to 16 Gi and a 12 Gi tmp emptyDir.

> Status: scaffolded and env-gated. The full in-cluster build is heavy (turbopack, large dep tree)
> and has **not yet been run end-to-end** — first deploy should be watched and iterated (build
> memory, any standalone-incompatible imports, and the env-secret contents).
