# Contact delivery v1

The optional contact API delivers transactional email through SendGrid and SMS through Twilio. It deliberately does **not** extend `PushJob`: push capabilities, recipient addresses, phone numbers, sender identities, and provider credentials have different validation and lifecycle requirements.

## Endpoints

- `GET /v1/contact/readyz`
- `POST /v1/contact/jobs`
- `POST /v1/contact/jobs/batch`

All submission routes use the same fail-closed HTTP authenticator as push ingestion. No provider credential or sender identity is accepted in a request.

## Email request

```json
{
  "version": "v1",
  "job_id": "job-123",
  "tenant_id": "tenant-1",
  "application_id": "app-1",
  "idempotency_key": "order-123-email",
  "provider": "sendgrid",
  "target": {
    "type": "email",
    "address": "person@example.invalid",
    "name": "Person"
  },
  "content": {
    "channel": "email",
    "subject": "Your order shipped",
    "text": "Your order is on its way.",
    "html": "<p>Your order is on its way.</p>",
    "reply_to": "support@example.invalid"
  },
  "trace": {
    "correlation_id": "request-123"
  }
}
```

Dynamic templates are a separate, mutually exclusive mode:

```json
{
  "content": {
    "channel": "email",
    "template_id": "d-template-identifier",
    "dynamic_template_data": {
      "display_name": "Person",
      "order_reference": "order-123"
    }
  }
}
```

The server supplies the verified `from` email/name. Producers cannot override it. V1 intentionally excludes arbitrary headers, attachments, BCC, categories, and ASM group selection until tenant authorization and size accounting are defined.

## SMS request

```json
{
  "version": "v1",
  "job_id": "job-124",
  "tenant_id": "tenant-1",
  "application_id": "app-1",
  "idempotency_key": "order-123-sms",
  "provider": "twilio",
  "target": {
    "type": "sms",
    "e164": "+15550001111"
  },
  "content": {
    "channel": "sms",
    "body": "Your order is on its way."
  },
  "trace": {
    "correlation_id": "request-124"
  }
}
```

Phone numbers must be E.164 and SMS content is capped at 1,600 Unicode characters. The server supplies either a Messaging Service SID or an approved `From` number. Producers cannot choose a sender.

## Immediate outcomes

`ContactOutcome` contains:

- version and job ID
- provider
- non-reversible target fingerprint
- normalized class
- optional provider request/message ID
- optional retry delay
- bounded safe detail

Classes are:

- `accepted`
- `invalid_target`
- `invalid_payload`
- `throttled`
- `transient_provider_failure`
- `permanent_provider_failure`
- `internal_failure`

`accepted` means SendGrid or Twilio accepted the API request. It does **not** mean the recipient received the message. Final delivery, bounce, block, unsubscribe, failed, delivered, or read state belongs to signature-verified provider callbacks and a separate durable event contract.

## Configuration

SendGrid:

- `SENDGRID_API_KEY`
- `SENDGRID_FROM_EMAIL`
- `SENDGRID_FROM_NAME` (optional)
- `SENDGRID_REGION=global|eu`
- `SENDGRID_SANDBOX_MODE=true|false`

Twilio:

- `TWILIO_ACCOUNT_SID`
- exactly one credential mode:
  - `TWILIO_AUTH_TOKEN`, or
  - `TWILIO_API_KEY_SID` plus `TWILIO_API_KEY_SECRET`
- exactly one sender:
  - `TWILIO_MESSAGING_SERVICE_SID`, or
  - `TWILIO_FROM_NUMBER`
- `TWILIO_STATUS_CALLBACK_URL` (optional HTTPS URL)
- `TWILIO_VALIDITY_PERIOD_SECONDS` (optional, 1–36000)

Partially configured providers fail startup instead of silently accepting requests that cannot be delivered.

## Security invariants

- Recipient email addresses and phone numbers never appear in normalized outcomes.
- Raw upstream response bodies are never echoed to callers.
- Provider credentials and sender identities are server-side only.
- Provider error details are classified into bounded codes/messages.
- Email explicit-content and dynamic-template modes cannot be mixed.
- Contact readiness is separate from push readiness because both lanes are optional and independently deployable.
- Delivery callbacks must be signature verified before they can alter durable delivery state.
