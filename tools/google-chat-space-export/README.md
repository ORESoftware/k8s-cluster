# Google Chat HTTP bridge

Account-side Google Apps Script bridge for one fixed Chat space:

- display name: `alex-alex-me`
- space resource: `spaces/AAQAoHKdzvI`
- source: <https://chat.google.com/room/AAQAoHKdzvI?cls=5>
- earliest message: **May 10, 2026 at 00:00 America/New_York**

The bridge does not scrape Google Chat. It runs as the deploying Google user and calls the read-only Google Chat API.

## Security

- Fixed space and fixed earliest timestamp; callers cannot widen either boundary.
- Only read-only Chat scopes plus Apps Script's send-mail scope are requested.
- A high-entropy bridge token gates every sensitive HTTP action.
- Only the token's SHA-256 hash is stored in Script Properties.
- POST is preferred. GET query tokens can appear in URL logs, so rotate the token after an import.
- Message text and Google OAuth credentials are not written to logs or Script Properties.
- Attachment metadata is preserved, but binaries are not downloaded.
- The Gmail export recipient is fixed to `alexander.d.mills@gmail.com`.

## Install

1. Sign into the Google account that belongs to `alex-alex-me`.
2. Open <https://script.new>.
3. Add [`App.gs`](./App.gs) and [`EmailExport.gs`](./EmailExport.gs).
4. In **Project Settings**, enable **Show `appsscript.json` manifest file in editor** and replace it with [`appsscript.json`](./appsscript.json).
5. Link a standard Google Cloud project and enable the **Google Chat API**.
6. In **Services**, add **Google Chat API v1** with identifier `Chat`.
7. Run `setupBridge()` manually and approve access. Copy `CHAT_BRIDGE_TOKEN` from the execution log.
8. Deploy as **Web app**:
   - execute as: **Me**
   - access: **Anyone**
9. Keep the `/exec` deployment URL and bridge token private.

## Preferred connected-Gmail export

Run `startEmailGoogleChatExport()` manually from the Apps Script editor. The function:

- probes access to the fixed Chat space;
- retrieves 100 messages per page, beginning May 10, 2026;
- sends each page as a JSON attachment to `alexander.d.mills@gmail.com`;
- schedules `continueEmailGoogleChatExport()` once per minute until pagination completes;
- stores only pagination/count metadata in Script Properties;
- sends a final `COMPLETE` summary email.

The subject format is:

```text
[Google Chat export AAQAoHKdzvI] run <RUN_ID> part <00001>
```

Useful editor functions:

- `getEmailGoogleChatExportStatus()` — non-sensitive progress metadata
- `cancelEmailGoogleChatExport()` — stops future continuation triggers
- `clearEmailGoogleChatExportState()` — clears pagination state after reconciliation

A rare crash after sending but before state persistence can resend a part. Consumers must deduplicate by `runId`, `partNumber`, filename, and the attachment's `dedupeKey`; resending is safer than silently skipping messages.

Adding the `script.send_mail` scope can cause Google to request one additional authorization approval. Updating editor functions does not require redeploying the web app unless the HTTP behavior changes.

## HTTP API

All responses are JSON. Apps Script ContentService returns HTTP 200 even for application-level failures, so check the top-level `ok` field.

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

Use the returned `data.nextPageToken` as `pageToken` on the next request. Supported authenticated actions are `status`, `probe`, `space`, and `messages`.

After the import, run `rotateBridgeToken()` or `disableBridge()` from the editor.

## Linear import rules

Group messages by thread and work item, search completed and archived Linear issues, persist each deterministic `sourceKey`, and add context to existing issues instead of creating duplicates. Tracking issue: `DEN-266` in `github.com/ORESoftware`.
