# `dd-patent-filing-rs`

Rust/Axum patent filing preparation service.

The service helps an operator turn invention intake into a draft filing package:

- guided HTML intake at `/` using htmx form posts
- JSON provisional-package generation at `/packages/provisional`
- readiness scoring at `/readiness`
- prior-art search planning at `/search/plan`
- package review at `/review/package`
- generated API docs at `/docs/api`, `/api/docs`, and `/api/docs.json`

It prepares package evidence and checklists only. It does not hold USPTO Patent Center
credentials, file applications, give legal advice, or replace attorney review.

## Routes

- `GET /` - htmx home page and package generator.
- `GET /descriptor` - service descriptor.
- `GET /schema` - request/response contract summary.
- `GET /example` - sample intake request.
- `GET /matters` - recent in-memory matters.
- `GET /matters/:matter_id` - one generated matter package.
- `POST /packages/provisional` - authenticated JSON package generation.
- `POST /ui/packages` - authenticated htmx form package generation.
- `POST /readiness` - authenticated readiness review from intake.
- `POST /search/plan` - authenticated prior-art search plan from intake.
- `POST /review/package` - authenticated review of a generated package.
- `GET /healthz`, `GET /readyz`, `GET /metrics`.
- `GET /docs/api`, `GET /api/docs`, `GET /api/docs.json`.

Mutating and sensitive routes require `X-Server-Auth` or `Auth` to match
`SERVER_AUTH_SECRET`, unless `PATENT_FILING_ALLOW_UNAUTHENTICATED=true` is set for
local development.

## Environment

| Var | Default | Purpose |
| --- | --- | --- |
| `HOST` | `0.0.0.0` | Bind host. |
| `PORT` | `8116` | Bind port. |
| `SERVER_AUTH_SECRET` / `PATENT_FILING_SERVER_AUTH_SECRET` | unset | Required shared secret for package generation and matter reads. |
| `PATENT_FILING_ALLOW_UNAUTHENTICATED` | `false` | Local-dev escape hatch. |
| `PATENT_FILING_CENTER_URL` | `https://patentcenter.uspto.gov/` | Operator handoff URL surfaced in generated checklists. |
| `PATENT_FILING_MAX_MATTERS` | `200` | In-memory matter retention cap. |

## Local Smoke

```bash
cd remote/deployments/patent-filing-rs
PATENT_FILING_ALLOW_UNAUTHENTICATED=true cargo run
```

Open `http://127.0.0.1:8116/` and submit the prefilled intake form.
