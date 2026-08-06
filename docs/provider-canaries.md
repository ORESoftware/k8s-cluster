# Provider canaries

The normal test suite uses deterministic mock provider endpoints and real process/JetStream transports. It never needs production credentials and never contacts SendGrid, Twilio, Apple, Google, Expo, or browser push services.

For an additional provider-contract check, the `Provider canaries` workflow can be started manually after repository secrets are configured.

## SendGrid sandbox canary

Required secrets:

- `SENDGRID_CANARY_API_KEY`: a least-privilege key with Mail Send permission
- `SENDGRID_CANARY_FROM_EMAIL`: a verified sender
- `SENDGRID_CANARY_TO_EMAIL`: a syntactically valid canary recipient

Optional repository variable:

- `SENDGRID_CANARY_REGION`: `global` or `eu`; empty defaults to `global`

The test uses SendGrid sandbox mode. SendGrid validates the Mail Send payload but does not deliver email, consume credits, or emit Email Activity/Event Webhook events.

## Twilio test-credential canary

Required secrets:

- `TWILIO_TEST_ACCOUNT_SID`
- `TWILIO_TEST_AUTH_TOKEN`
- `TWILIO_TEST_TO_NUMBER`

The test uses Twilio's documented valid magic From number with account test credentials. Twilio returns a realistic Messages API response without billing, mutating production state, or connecting to a carrier.

## Safety

- The workflow runs on `workflow_dispatch` and a weekly schedule against `main`; it is not run for arbitrary pull requests.
- It skips cleanly when the relevant secret set is absent.
- Credentials are supplied only through GitHub Actions secrets.
- Test jobs assert that normalized outcomes contain fingerprints rather than recipient addresses or phone numbers.
- These canaries prove provider request compatibility, not final delivery. Signed provider callbacks and durable reconciliation are tracked separately.
