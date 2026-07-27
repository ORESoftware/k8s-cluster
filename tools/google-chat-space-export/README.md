# Google Chat HTTP bridge

Account-side Google Apps Script bridge for one fixed Chat space:

- display name: `alex-alex-me`
- space resource: `spaces/AAQAoHKdzvI`
- source: <https://chat.google.com/room/AAQAoHKdzvI?cls=5>
- earliest message: **May 10, 2026 at 00:00 America/New_York**

The bridge does not scrape Google Chat. It runs as the deploying Google user, calls the read-only Google Chat API, and returns paginated JSON through an Apps Script web-app endpoint.

## Security

- Fixed space and fixed earliest timestamp; HTTP callers cannot widen either boundary.
- Only `chat.messages.readonly` and `chat.spaces.readonly` are requested.
- A high-entropy bridge token gates every sensitive action.
- Only the token's SHA-256 hash is stored in Script Properties.
- POST is preferred. GET is available for restricted clients but query tokens can appear in URL logs, so rotate the token after an import.
- Message text and Google OAuth credentials are not written to logs or Script Properties.
- Attachment metadata is returned, but binaries are not downloaded.
- Best-effort global rate limiting and script locks protect the endpoint.

## Install

1. Sign into the Google account that belongs to `alex-alex-me`.
2. Open <https://script.new>.
3. Replace the default file with [`App.gs`](./App.gs).
4. In **Project Settings**, enable **Show `appsscript.json` manifest file in editor** and replace it with [`appsscript.json`](./appsscript.json).
5. Link a standard Google Cloud project and enable the **Google Chat API**.
6. In **Services**, add **Google Chat API v1** with identifier `Chat`.
7. Run `setupBridge()` manually and approve access. Copy `CHAT_BRIDGE_TOKEN` from the execution log.
8. Deploy as **Web app**:
   - execute as: **Me**
   - access: **Anyone**
9. Keep the `/exec` deployment URL and bridge token private.

## HTTP API

All responses are JSON. Apps Script ContentService returns HTTP 200 even for application-level failures, so check the top-level `ok` field.

The deployment is smoke-tested from GitHub Actions so the public health route and unauthenticated boundary are verified from outside the local execution network. During rollout validation, that smoke test runs on each normal PR update.

Public health check:

```text
GET <EXEC_URL>?action=health
```

Preferred authenticated POST:

```bash
curl -sS -X POST '<EXEC_URL>' \
  -H 'content-type: application/json' \
  --data '{"action":"messages","token":"<TOKEN>","pageSize":100}'
```

GET fallback:

```text
GET <EXEC_URL>?action=messages&token=<TOKEN>&pageSize=100
```

Use the returned `data.nextPageToken` as `pageToken` on the next request. Supported authenticated actions are `status`, `probe`, `space`, and `messages`. `messages` also accepts `metadataOnly`, `showDeleted`, and a validated `threadName` from this exact space.

After the import, run `rotateBridgeToken()` or `disableBridge()` from the editor.

## Linear import rules

The downstream importer should group messages by thread and work item, search completed and archived Linear issues, persist the deterministic `sourceKey`, and add context to existing issues instead of creating duplicates. Tracking issue: `DEN-266` in `github.com/ORESoftware`.
