# Founder Partnership Control Plane — initial provider connector plan

**Status:** Discovery / sandbox design  
**Linear:** DEN-158, DEN-178  
**Providers:** GitHub organizations and Cloudflare DNS

## Decision

Build the first real connector slice around:

1. **GitHub organization rulesets** using a GitHub App installation credential.
2. **Cloudflare DNS records** using a narrowly scoped API token.

Do not initially execute organization-owner, Cloudflare Super Administrator,
registrar unlock, authorization-code, domain transfer, billing, or account-token
administration actions. Treat those as constitutional or root-control surfaces:
monitor them, alert on drift, and require a manual/legal/provider-specific
procedure until custody and bypass behavior are proven.

This sequence provides a meaningful enforcement test without placing production
ownership or domain registration at risk.

## Why these two actions

### GitHub organization rulesets

GitHub's current REST API supports reading, creating, updating, deleting, and
inspecting history for organization rulesets. Fine-grained GitHub App
installation tokens can call the ruleset endpoints when the installation has the
required organization Administration permission.

Rulesets are valuable for the prototype because they can protect many
repositories while remaining reversible in a sandbox organization. They also
expose readable current state and version history for reconciliation.

Official references:

- <https://docs.github.com/en/rest/orgs/rules>
- <https://docs.github.com/en/rest/authentication/endpoints-available-for-github-app-installation-access-tokens>
- <https://docs.github.com/en/rest/apps/apps>

### Cloudflare DNS

Cloudflare exposes list, get, create, update, overwrite, and delete DNS-record
endpoints. Cloudflare recommends API tokens, and DNS read/write can be scoped to
a particular zone.

DNS changes provide a realistic high-impact connector while still allowing a
dedicated sandbox zone and narrow token. Current state is directly readable for
post-write reconciliation and drift polling.

Official references:

- <https://developers.cloudflare.com/api/resources/dns/subresources/records/>
- <https://developers.cloudflare.com/api/resources/dns/subresources/records/methods/list/>
- <https://developers.cloudflare.com/api/resources/dns/subresources/records/methods/edit/>
- <https://developers.cloudflare.com/api/resources/dns/subresources/records/methods/delete/>

## Root-control findings

### GitHub ownership

The organization-membership API can assign the `admin` role, which makes a user
an organization owner, when the caller has the required Members permission.
That makes owner changes technically addressable, but it also makes them far too
powerful for the first connector.

Owner and membership drift should be polled from the organization-members API,
including role and 2FA-related filters where available. Membership changes must
remain constitutional in the Fiducia action taxonomy.

References:

- <https://docs.github.com/en/rest/orgs/members>
- <https://docs.github.com/en/rest/orgs/organization-roles>

### Cloudflare Super Administrator and registrar

Cloudflare's Super Administrator role can manage members, billing, and
account-owned API tokens, and one Super Administrator can revoke another.
Cloudflare also warns that Super Administrators and zone administrators can
unlock Registrar domains and obtain transfer authorization codes.

Therefore:

- Cloudflare account membership and role changes are constitutional.
- Account-owned API-token administration is constitutional.
- Registrar unlock, transfer authorization code retrieval, transfer-out, and
  inter-account transfer are constitutional and monitor/manual-only initially.
- A DNS-only token cannot protect against a human Super Administrator changing
  the same zone through the dashboard or minting another token; drift detection
  is mandatory.

References:

- <https://developers.cloudflare.com/fundamentals/manage-members/roles/>
- <https://developers.cloudflare.com/fundamentals/account/change-super-admin/>
- <https://developers.cloudflare.com/registrar/account-options/transfer-out-from-cloudflare/>
- <https://developers.cloudflare.com/registrar/account-options/inter-account-transfer/>

## Credential custody

### GitHub

Use a dedicated GitHub App installed only on the sandbox organization and
repositories needed by the connector.

- Store the App private key in KMS/HSM-backed custody when the runtime supports
  it.
- Mint an installation access token only after Fiducia authorization.
- Installation tokens are short-lived; never persist them as durable tenant
  secrets.
- Restrict installation permissions to the smallest set required for rulesets,
  current-state reads, and audit/drift signals.
- Do not reuse a founder's personal access token as the executor credential.
- Record the installation ID, permission snapshot, token issue/expiry, proposal
  hash, and fencing generation without logging the token.

### Cloudflare

Use a dedicated token limited to DNS read/write for one sandbox zone.

- Prefer an account-owned token when operationally available, but do not grant
  account-member, account-token, billing, or registrar permissions.
- Store the bearer token encrypted and expose it only to the fenced executor.
- Rotate on a short schedule and immediately after suspected disclosure.
- Poll account-owned token inventory with a separate read-only monitoring
  credential where possible.
- Never use the Global API key for the connector.

Cloudflare bearer tokens are exportable secrets, not inherently non-exportable
HSM keys. The deployment must clearly distinguish encrypted storage from true
non-exportable signing custody.

## Initial action taxonomy

### GitHub

| Fiducia action kind | Default class | Initial support |
| --- | --- | --- |
| `github.ruleset.read` | routine | Read/reconcile |
| `github.ruleset.create_evaluate` | routine | Sandbox only |
| `github.ruleset.update_evaluate` | routine | Sandbox only |
| `github.ruleset.enforce` | sensitive | Strong quorum and cooldown |
| `github.ruleset.delete` | sensitive | Strong quorum; prefer disable/evaluate first |
| `github.member.read_admins` | routine | Drift monitoring |
| `github.member.promote_owner` | constitutional | Monitor/manual only |
| `github.member.remove_owner` | constitutional | Monitor/manual only |
| `github.organization.update_root_settings` | constitutional | Not supported initially |

### Cloudflare

| Fiducia action kind | Default class | Initial support |
| --- | --- | --- |
| `cloudflare.dns.read` | routine | Read/reconcile |
| `cloudflare.dns.create_sandbox_record` | routine | Sandbox zone only |
| `cloudflare.dns.update_sandbox_record` | sensitive | Strong quorum for protected names |
| `cloudflare.dns.delete_sandbox_record` | sensitive | Strong quorum and cooldown |
| `cloudflare.member.read` | routine | Drift monitoring |
| `cloudflare.member.change_super_admin` | constitutional | Monitor/manual only |
| `cloudflare.account_token.create_or_revoke` | constitutional | Monitor/manual only |
| `cloudflare.registrar.unlock_or_transfer` | constitutional | Monitor/manual only |

Unrecognized provider methods fail closed. Connector code must map every outbound
request to a registered action kind rather than accept an arbitrary URL, method,
or payload.

## Canonical parameters

### GitHub ruleset proposal

Commit at least:

```yaml
organization: fiducia-sandbox
ruleset_id: optional-existing-id
ruleset_name: fiducia-proposal-<proposal-id>
target: branch
enforcement: evaluate | active
conditions_hash: sha256:...
rules_hash: sha256:...
bypass_actors_hash: sha256:...
expected_ruleset_version: optional-version
```

The connector must not accept arbitrary JSON after approval. It must regenerate
the provider payload from validated canonical parameters.

### Cloudflare DNS proposal

Commit at least:

```yaml
account_id: account-id
zone_id: sandbox-zone-id
record_id: optional-existing-id
record_type: A | AAAA | CNAME | TXT | ...
record_name: canary.example.test
record_content_hash: sha256:...
ttl: 300
proxied: false
expected_record_version_hash: sha256:...
```

Secret TXT contents should be encrypted or referenced, with only their hash in
the approval and audit envelope where practical.

## Idempotency and reconciliation

Neither connector may assume a timeout means the provider did nothing.

### Common algorithm

1. Persist the proposal-derived idempotency key and fencing claim.
2. Read current provider state.
3. If the desired state already exists and matches, emit a reconciled success
   receipt without mutating.
4. Perform one provider mutation.
5. On success, read back and compare canonical state.
6. On timeout/connection loss, read back and reconcile before any retry.
7. If state cannot be determined, keep the execution `unknown`, preserve the
   pending claim, freeze conflicting proposals, and alert an operator.

### GitHub provider key

Use a deterministic ruleset name or embedded metadata derived from the Fiducia
proposal ID. Before creating, list rulesets and reject conflicting names or
mismatched desired hashes. For updates, commit the expected current ruleset
version/hash and fail on drift.

### Cloudflare provider key

Use the stable DNS record ID for updates/deletes. For creates, use the canonical
zone/type/name tuple plus a Fiducia proposal marker in the record comment or tag
when supported. Read the exact name/type before and after mutation and reject
CNAME/A/AAAA or NS conflicts rather than silently replacing unrelated records.

## Drift detection

### GitHub

Poll and/or consume events for:

- organization rulesets, enforcement state, bypass actors, and version history;
- rule-suite evaluations;
- organization members with owner/admin role;
- members with disabled or insecure 2FA where available;
- GitHub App installation existence, permissions, and repository selection;
- organization role assignments;
- repository visibility, branch protection, webhooks, deploy keys, and Actions
  settings added to later connector phases.

GitHub organization audit logs and webhooks can supplement polling. Audit-log
availability and retention differ by plan and event type, so current-state reads
remain the source for enforcement reconciliation.

References:

- <https://docs.github.com/en/organizations/keeping-your-organization-secure/managing-security-settings-for-your-organization/audit-log-events-for-your-organization>
- <https://docs.github.com/en/organizations/keeping-your-organization-secure/managing-security-settings-for-your-organization/reviewing-the-audit-log-for-your-organization>

### Cloudflare

Poll:

- exact DNS records and nameserver/delegation state;
- account members, roles, and status;
- account-owned API-token inventory;
- zone settings relevant to the protected record;
- domain/registrar lock state through a separate manual or supported read path.

Consume Cloudflare Audit Logs v2 for mutations and actor context. Audit logs are
an evidence source, not the sole drift mechanism: current documentation notes
that some views and failed requests are not logged, so polling exact current
state is still required.

References:

- <https://developers.cloudflare.com/api/resources/accounts/subresources/logs/subresources/audit/methods/list/>
- <https://developers.cloudflare.com/api/resources/accounts/subresources/members/>
- <https://developers.cloudflare.com/api/resources/accounts/subresources/tokens/methods/list/>

## Sandbox topology

### GitHub

- Dedicated test organization or explicitly disposable repositories.
- Two human test owners remain available for recovery.
- Dedicated Fiducia GitHub App with organization Administration permission only
  where required.
- Rulesets target only branches/repositories labeled for the experiment.
- Start with `evaluate` enforcement.
- Never remove the final human owner or modify production organizations.

### Cloudflare

- Dedicated test account or isolated sandbox zone.
- Domain/zone with no production traffic, email, authentication, or certificate
  dependencies.
- DNS token scoped to that one zone.
- Canary record under an isolated subdomain.
- Short TTL and documented rollback value.
- No registrar or member-management permission.

## Sandbox acceptance tests

1. Unsatisfied Fiducia quorum produces zero provider requests.
2. Approved GitHub `evaluate` ruleset is created exactly once.
3. Parameter mutation after approval is rejected locally.
4. A simulated timeout after provider mutation reconciles to one success.
5. A stale fencing token cannot update or delete the ruleset/record.
6. A second proposal cannot overwrite provider drift without a new approval.
7. Cloudflare DNS create/update/delete is limited to the configured zone/name
   allowlist.
8. Continuity delegated authority can perform a bounded canary DNS/ruleset action
   but cannot enforce a constitutional owner/member/registrar action.
9. Manual dashboard drift produces an alert and freezes conflicting automated
   execution.
10. Revoking the App installation or API token causes fail-closed behavior and a
    credential-health alert.
11. Audit receipts contain hashes and provider request IDs but no credentials.
12. Rollback is itself a new policy-gated proposal unless an exact pre-approved
    defensive rollback capability exists.

## Provider-support bypasses

The product must display these residual risks explicitly:

- A GitHub human owner may use dashboard, personal credentials, or support paths
  outside the App's control.
- A Cloudflare Super Administrator may alter members, tokens, DNS, or registrar
  state outside the DNS token.
- Cloudflare zone administrators may have registrar transfer powers for domains
  registered through Cloudflare.
- Provider support may restore accounts or change access through processes that
  Fiducia cannot cryptographically veto.

Fiducia mitigates these paths with least privilege, independent notices,
continuous drift detection, audit export, rapid freeze/revocation, and legal
agreements. It must not market them as mathematically impossible.

## Implementation sequence

1. Implement read-only GitHub ruleset and Cloudflare DNS snapshots.
2. Normalize provider state and compute canonical hashes.
3. Add drift comparison and alert receipts.
4. Add sandbox create/update using the existing executor model.
5. Add timeout reconciliation and stale-fencing tests with fake transports.
6. Run against disposable real provider resources.
7. Add sensitive enforcement/delete paths only after rollback drills.
8. Keep owner/member/registrar actions disabled until separate constitutional
   design, credential custody, and counsel/provider reviews are complete.
