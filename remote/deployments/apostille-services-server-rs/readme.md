# dd-apostille-services-server

Rust Axum service for apostille, notary, immigration, translation, configured
government-provider submissions, and inbound status webhooks.

The service intentionally does not hard-code live government APIs. Government
integrations are opt-in connectors configured from operator-owned environment
variables or secret-backed JSON. That keeps source control free of credentials
and avoids pretending every jurisdiction exposes the same API shape.

## Home Page

The service has a public HTML landing page at:

- `GET /`
- `GET /home`

It uses HTMX fragments for:

- `GET /home/flow` - workflow explanation.
- `GET /home/jurisdictions` - jurisdiction catalog and connector state.
- `GET /home/webhooks` - webhook URL, auth headers, and callback envelope.
- `GET /home/examples` - sample case and provider config payloads.

## API Endpoints

- `GET /descriptor` - service descriptor, auth model, connectors, and endpoints.
- `GET /jurisdictions` - Peru, Colombia, USA, Mexico, Europe/EU, Spain, Italy, China, Taiwan, Thailand, Cambodia, France, Croatia, Laos, Myanmar/Burma, Vietnam, and Brazil/Brasil.
- `GET /services` - apostille/legalization, notary, and immigration service types.
- `GET /schema` - request, connector, and webhook contract summary.
- `GET /example` - sample JSON payloads.
- `GET /cases` - authenticated in-memory case ledger.
- `POST /cases` - authenticated normalized case intake. When `workflow.autoSubmit=true`, consent is complete, translations are complete, and a provider connector exists, it submits automatically.
- `GET /cases/:case_id` - authenticated case detail.
- `POST /cases/:case_id/submit` - authenticated provider submission.
- `POST /translate` - authenticated ad-hoc translation request.
- `POST /webhooks/government` - provider status webhook.
- `POST /webhooks/government/:jurisdiction` - provider status webhook with path-level jurisdiction.
- `GET /healthz`, `GET /readyz`, `GET /metrics` - runtime probes.
- `GET /docs/api`, `GET /api/docs`, `GET /api/docs.json` - generated API docs.

Operator routes accept `X-Server-Auth` or the legacy `Auth` header matching
`SERVER_AUTH_SECRET`, unless `APOSTILLE_ALLOW_UNAUTHENTICATED=true` is set for
local development.

Government callback routes use `APOSTILLE_WEBHOOK_SECRET` and one of:

- `X-Apostille-Webhook-Secret`
- `X-Government-Webhook-Secret`
- `X-Webhook-Secret`

If `APOSTILLE_WEBHOOK_SECRET` is unset, webhook routes fall back to operator
auth. Do not expose webhook routes publicly without a webhook secret.

## Provider Connectors

The preferred connector config is a JSON object in
`APOSTILLE_PROVIDER_CONFIG_JSON`; tokens are still read from env var names so
the JSON can be documented without containing secrets:

```json
{
  "peru": {
    "baseUrl": "https://approved-provider.example",
    "submitPath": "/apostille/submit",
    "statusPath": "/apostille/status",
    "services": ["apostille", "notary", "immigration"],
    "authKind": "bearer",
    "tokenEnv": "APOSTILLE_PROVIDER_PERU_TOKEN"
  }
}
```

Per-jurisdiction env vars are also supported:

- `APOSTILLE_PROVIDER_PERU_BASE_URL`
- `APOSTILLE_PROVIDER_PERU_SUBMIT_PATH`
- `APOSTILLE_PROVIDER_PERU_STATUS_PATH`
- `APOSTILLE_PROVIDER_PERU_SERVICES`
- `APOSTILLE_PROVIDER_PERU_AUTH_KIND`
- `APOSTILLE_PROVIDER_PERU_AUTH_HEADER`
- `APOSTILLE_PROVIDER_PERU_TOKEN`

Replace `PERU` with the uppercased jurisdiction slug, using underscores for
hyphens. Provider URLs must be `http` or `https`, must not embed credentials,
and block private/local hosts by default. Only set
`APOSTILLE_ALLOW_PRIVATE_PROVIDER_URLS=true` for approved private-network
integrations.

## Translation

Set `APOSTILLE_TRANSLATION_BASE_URL` to translate non-English document titles
and text to the target language, normally English:

- `APOSTILLE_DEFAULT_TARGET_LANGUAGE` - default `en`.
- `APOSTILLE_TRANSLATION_BASE_URL`
- `APOSTILLE_TRANSLATION_PATH` - default `/translate`.
- `APOSTILLE_TRANSLATION_AUTH_HEADER` - default `authorization`.
- `APOSTILLE_TRANSLATION_AUTH_TOKEN`

The translation provider is expected to accept:

```json
{
  "requestId": "case_...",
  "documentId": "doc_1",
  "field": "text",
  "text": "...",
  "sourceLanguage": "es",
  "targetLanguage": "en",
  "context": "apostille"
}
```

and return one of `translatedText`, `translation`, or `text`.

## Local Development

```sh
cd remote/deployments/apostille-services-server-rs
APOSTILLE_ALLOW_UNAUTHENTICATED=true cargo run
```

Then open `http://127.0.0.1:8122/`.

Generate API docs from source route declarations:

```sh
node ../../tools/generate-api-docs.mjs
```

Secrets belong in AWS Secrets Manager / Kubernetes secrets, not Git.
