# Google Chat reconciliation pipeline

The Google Chat export is private input. The only supported path into the deterministic Linear planner is:

1. fetch raw bridge pages with a token supplied through the environment;
2. sanitize every page and quarantine secret-bearing or private-contact content;
3. inspect the value-free safety report;
4. run the planner against the sanitized directory with an explicit reconciliation window;
5. apply reviewed actions in bounded batches;
6. rerun the planner and require zero duplicate issue creations;
7. rotate or disable the bridge token.

Raw message bodies, attachment metadata from quarantined messages, credential values, phone numbers, and private operational addresses must never be committed, copied into Linear, or included in pull-request descriptions.

## Fetch

```bash
umask 077
export CHAT_BRIDGE_TOKEN='<operator-provided token>'
node tools/google-chat-space-export/fetch-bridge-pages.mjs \
  --out ./private/google-chat-raw
unset CHAT_BRIDGE_TOKEN
```

The fetcher sends the token only in the original POST body. Google Apps Script redirects the request to a `googleusercontent.com` response URL; the client follows that redirect as GET rather than reposting the credential.

## Sanitize

```bash
node tools/google-chat-space-export/sanitize-export.mjs \
  --input ./private/google-chat-raw \
  --out ./private/google-chat-sanitized \
  --since 2026-06-05T04:00:00.000Z \
  --report ./private/google-chat-safety-report.json
```

The sanitizer keeps source keys, timestamps, thread routing, and non-content provenance. For a quarantined message it clears text-like fields and attachment/annotation arrays, then records only classification and finding kinds. The report contains no matched value or message body.

`--fail-on-sensitive` is useful in automation that must stop when any secret-bearing message is present, but quarantine still occurs before the process exits with status 2.

## Plan

```bash
node tools/google-chat-space-export/import-plan.mjs \
  --input ./private/google-chat-sanitized \
  --since 2026-06-05T04:00:00.000Z \
  --existing-index ./private/linear-issue-index.json \
  --project-map tools/google-chat-space-export/import-project-map.example.json \
  --json ./private/google-chat-import-plan.json \
  --markdown ./private/google-chat-import-plan.md
```

`--since` may only narrow the fixed May 10 bridge boundary. Deduplication is computed across all supplied messages, while `plannedMessages` and `windowedOutMessages` describe the selected window.

Do not point the planner directly at the raw export. The integration test `reconciliation-pipeline.test.mjs` proves that quarantined secrets and contact values are absent from both the safety report and resulting plan while deterministic provenance survives.

## Apply and close out

Review every `create`, `comment-existing`, `manual-review`, and `skip-non-actionable` candidate. Persist deterministic source keys on the canonical Linear issue. After the controlled apply, regenerate the issue index and rerun the same plan command; duplicate creation count must be zero.

Finally run `rotateBridgeToken()` or `disableBridge()` in Apps Script and delete the private raw/sanitized working directories through the operator's approved secure-cleanup process.
