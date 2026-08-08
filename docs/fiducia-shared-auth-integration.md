# Fiducia and Shared Auth identity boundaries

Fiducia customer and operator traffic are separate security planes. The customer
application and admin application must use different Supabase projects even
though each process keeps the same standard runtime variable names:

| Workload | Kubernetes Secret | Remote object | Required remote properties |
|---|---|---|---|
| `fiducia-backend` | `fiducia-backend-secrets` | `dd/remote-dev/fiducia-backend-secrets` | `FIDUCIA_CUSTOMER_SUPABASE_URL`, `FIDUCIA_CUSTOMER_SUPABASE_PUBLISHABLE_KEY` |
| `fiducia-admin` | `fiducia-admin-secrets` | `dd/remote-dev/fiducia-admin-secrets` | `FIDUCIA_ADMIN_SUPABASE_URL`, `FIDUCIA_ADMIN_SUPABASE_PUBLISHABLE_KEY` |

External Secrets maps each plane-specific remote property into the target key
`supabase-url` or `supabase-publishable-key`. The deployments then expose those
as `SUPABASE_URL` and `SUPABASE_PUBLISHABLE_KEY` inside only that process. This
keeps application configuration conventional without allowing the two workload
manifests to reference one shared credential object.

`scripts/check-fiducia-auth-boundaries.py` is a structural CI guard. It proves
that the two deployments consume distinct target Secrets and that the external
secret contract uses plane-specific remote property names. It cannot read or
compare secret values. Before rollout, the operator must also verify that the two
URLs resolve to different Supabase project refs.

## Shared Auth registry

The centralized `dd-shared-auth` deployment accepts both projects in
`AUTH_SUPABASE_PROJECTS`:

```json
[
  {
    "name": "fiducia-customer",
    "project_ref": "<customer-ref>",
    "publishable_key_env": "AUTH_SUPABASE_FIDUCIA_CUSTOMER_PUBLISHABLE_KEY",
    "secret_key_env": "AUTH_SUPABASE_FIDUCIA_CUSTOMER_SECRET_KEY"
  },
  {
    "name": "fiducia-admin",
    "project_ref": "<admin-ref>",
    "publishable_key_env": "AUTH_SUPABASE_FIDUCIA_ADMIN_PUBLISHABLE_KEY",
    "secret_key_env": "AUTH_SUPABASE_FIDUCIA_ADMIN_SECRET_KEY"
  }
]
```

The metadata JSON belongs in `dd/shared-auth/supabase-projects`; referenced key
values and optional SendGrid/Twilio credentials belong in
`dd/shared-auth/provider-credentials`. No provider credential is committed to
Git or placed in a ConfigMap.

Shared Auth mirrors only verified provider identity into its own Postgres
`principals` and `provider_identities`, and owns local `sessions` and `roles`.
Supabase keeps its built-in auth schema and password behavior. Email equality is
not an account-linking rule.

## Rollout order

1. Create the new customer/admin plane-specific properties in their existing
   remote secret objects. Values must point to different Supabase projects.
2. Add both project records and referenced credentials to Shared Auth secret
   objects; apply the canonical `shared_auth` Postgres schema.
3. Run Shared Auth server and `shared-auth-lib` Fiducia contract tests.
4. Merge/sync the ExternalSecret change and confirm both generated Kubernetes
   Secrets are healthy before rolling either application.
5. Exercise customer login, admin login, cross-project rejection, Shared Auth
   exchange, SendGrid passwordless, and Twilio MFA with non-production accounts.

Do not merge the property rename before step 1: External Secrets will correctly
fail to materialize a missing property, and the applications will fail closed.
