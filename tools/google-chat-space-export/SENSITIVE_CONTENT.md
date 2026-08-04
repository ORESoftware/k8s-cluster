# Google Chat export safety gate

Run `sanitize-export.mjs` before any raw Google Chat export is passed to the
Linear import planner, copied into a report, or used to generate a pull request.
The sanitizer preserves source keys, timestamps, thread routing, and other
non-content metadata while quarantining message text and attachments when it
finds credential material or a high-confidence contact-only message.

```bash
node tools/google-chat-space-export/sanitize-export.mjs \
  --input ./private/raw-chat-export/pages \
  --out ./private/sanitized-chat-export \
  --since 2026-06-05T00:00:00Z \
  --report ./private/chat-safety-report.json

node tools/google-chat-space-export/import-plan.mjs \
  --input ./private/sanitized-chat-export \
  --existing-index ./private/linear-issue-index.json \
  --project-map tools/google-chat-space-export/import-project-map.example.json \
  --json ./private/google-chat-import-plan.json \
  --markdown ./private/google-chat-import-plan.md
```

The safety report contains only source keys, timestamps, classification names,
and secret kinds. It never contains message bodies or matched values. Raw and
sanitized exports are operational artifacts: do not commit either one.

A finding does not prove abuse. Revoke the exposed credential, review provider
audit logs from the first exposure timestamp, and replace it with a least-
privilege credential. Contact-only messages are quarantined for privacy and are
not engineering work items.
