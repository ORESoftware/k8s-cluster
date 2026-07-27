# Security audits

- `supabase_rls_audit.sql` validates owner-scoped Supabase Data API tables,
  policies, anonymous privileges, and exposed security-definer functions.
- `rds_role_audit.sql` validates TLS and the least-privilege posture of the exact
  Sonus RDS application/operator connection.
- `../scripts/audit_auth_storage.sh` runs either or both audits without printing
  connection strings.
- `../docs/auth-storage-security.md` defines the Supabase, shared-auth, Sonus RDS,
  and account-deletion trust boundaries.

The SQL is read-only. Use short-lived operator Postgres DSNs with catalog
visibility. Do not use Supabase service-role API keys, and do not place operator
DSNs in the serving Deployment.
