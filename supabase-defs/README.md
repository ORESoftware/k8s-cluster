# Supabase Definitions

`supabase-defs` contains project-specific Supabase contracts. It is deliberately
separate from [`pg-defs`](../pg-defs/):

- `pg-defs/schema/schema.sql` is the portable remote PostgreSQL contract.
- `supabase-defs/catalog.json` records the Supabase organization, project ref,
  project name, region, and schema-definition path.
- `supabase-defs/projects/<project-ref>/schemas/<schema>.sql` contains
  Supabase-only definitions such as `auth.users` foreign keys, RLS policies,
  grants to `authenticated`/`service_role`, and safe Data API projections.

The project ref is part of every definition path so a migration cannot be
silently applied to the wrong Supabase project. Project display names are
metadata only and are not assumed to be unique.

Run:

```sh
npm --prefix supabase-defs run check
npm --prefix supabase-defs test
```

## Sonus Auris cloud connections

The authoritative `sound_recorder_cloud_connections` row lives in remote
Postgres and may contain an AES-GCM token envelope. Supabase receives only the
owner-readable `public.cloud_connections` projection. That projection never
contains OAuth tokens, S3/R2 credentials, provider account identifiers, OAuth
scopes, or backend metadata.

Only the backend service role may write the projection. Authenticated clients
can select their own rows through RLS; they cannot forge connection state.
