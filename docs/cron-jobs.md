# Customer cron jobs

The customer portal exposes cron jobs under `/app/crons`. A verified customer
session chooses an organization the user actually belongs to; the BFF then
rebuilds a trusted request to `fiducia-node` or `fiducia-lambda-service` with the
canonical organization id. Browser cookies and bearer tokens are never
forwarded.

## Customer workflow

1. Create managed Node.js source as a draft function.
2. Run **Check & activate**. The lambda service validates the exact revision in
   its bounded sandbox before activation.
3. Create a UTC cron schedule whose function target is that opaque function
   UUID, or use a validated HTTPS webhook.
4. Pause, resume, delete, or trigger the schedule from the portal.
5. Open **Trail** to see terminal status, trigger type, attempts, duration, HTTP
   class, normalized error class, and trace id.

Function source remains in the function service's Postgres store. The replicated
scheduler stores only the opaque UUID and a bounded, sanitized run trail.

## HTTP surface

| Method | Path | Purpose |
| --- | --- | --- |
| GET, POST | `/api/customer/crons` | List or create schedules |
| GET, PUT, DELETE | `/api/customer/crons/:name` | Read, replace, or delete one schedule |
| POST | `/api/customer/crons/:name/pause` | Pause future scheduled claims |
| POST | `/api/customer/crons/:name/resume` | Resume a schedule |
| POST | `/api/customer/crons/:name/trigger` | Idempotent manual run |
| GET | `/api/customer/crons/:name/history` | Sanitized newest-first run trail |
| GET, POST | `/api/customer/cron-functions` | List or create managed function drafts |
| GET, PUT, DELETE | `/api/customer/cron-functions/:id` | Read, replace, or delete one tenant function |
| POST | `/api/customer/cron-functions/:id/check` | Check and activate the exact draft revision |
| POST | `/api/customer/cron-functions/:id/pause` | Disable invocation without deleting source |

Browser-session writes require the existing same-origin and CSRF contract plus
an `Idempotency-Key`. Non-browser API sessions require the API-host boundary and
the same idempotency key. The BFF forwards only:

- the configured trusted-hop credential;
- the canonical `x-fiducia-org-id`;
- the idempotency key for mutations;
- validated W3C `traceparent` and `tracestate` values.

Trace propagation is fail closed. `traceparent` must use the canonical version
`00` shape, lowercase hexadecimal fields, and nonzero trace and parent IDs.
`tracestate` is forwarded only when `traceparent` is valid, is capped at 512
bytes, and must contain printable ASCII. Malformed browser-supplied context is
dropped rather than becoming a trusted internal correlation identity.

The BFF never forwards `Cookie`, browser `Authorization`, function source into
logs, or raw upstream errors. Redirects are disabled, requests time out after
five seconds, and upstream responses are capped at two MiB.

## Configuration

- `FIDUCIA_CRON_NODE_URL` (or compatibility `FIDUCIA_NODE_URL`)
- `FIDUCIA_INTERNAL_SECRET`
- `FIDUCIA_LAMBDA_SERVICE_URL`
- `FIDUCIA_LAMBDA_SERVER_AUTH_SECRET`

A missing or malformed URL/secret disables that dependency and the corresponding
portal/API operation fails closed with `cron_service_not_configured`.

## Customer-code policy

The portal fixes runtime selection to managed `nodejs`. It rejects shell,
container, browser, arbitrary entry-command, and customer-supplied environment
configuration. Source is capped at 256 KiB and run time at 120 seconds. The
function service applies its stricter storage, activation, sandbox, output, and
invocation policies as the final authority.

## Verification

The reviewable branch contains only ordinary Rust source, documentation, and
tests. It must pass the repository's permanent format, strict Clippy, all-target
unit/integration test, CLI-contract, dependency-audit, and release workflows at
the exact final head. Temporary migration, diagnostic, encoded-payload, and
self-modifying workflow artifacts are not part of the implementation.
