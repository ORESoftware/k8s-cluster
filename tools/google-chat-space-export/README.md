# Google Chat HTTP bridge

Account-side Google Apps Script bridge for one fixed Chat space:

- display name: `alex-alex-me`
- space resource: `spaces/AAQAoHKdzvI`
- source: <https://chat.google.com/room/AAQAoHKdzvI?cls=5>
- earliest message: **May 10, 2026 at 00:00 America/New_York**

The bridge does not scrape Google Chat. It runs as the deploying Google user and calls the read-only Google Chat API.

## Security

- Fixed space and fixed earliest timestamp; callers cannot widen either boundary.
- Only read-only Chat scopes plus Apps Script's send-mail and trigger-management scopes are requested.
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

## Automatic clasp synchronization

The workflow [`.github/workflows/google-chat-apps-script-sync.yml`](../../.github/workflows/google-chat-apps-script-sync.yml) makes GitHub `main` the source of truth.

On changes under this directory, it:

1. validates the Apps Script manifest, JavaScript syntax, and fixed safety boundaries;
2. authenticates clasp without printing credentials;
3. pushes `App.gs`, `EmailExport.gs`, and `appsscript.json` with `clasp push --force`;
4. creates an immutable Apps Script version;
5. redeploys the existing deployment ID;
6. verifies the public `/exec?action=health` response.

Configure the GitHub environment **`google-chat-apps-script`** with these secrets:

- `CLASPRC_JSON`: complete contents of `~/.clasprc.json` produced by `clasp login` for the Google account that owns the Apps Script project.
- `CLASP_JSON`: complete contents of `.clasp.json` for this project, including its **Script ID**. The Script ID is available in Apps Script under **Project Settings → IDs**.

Before generating `CLASPRC_JSON`, enable the Apps Script API at <https://script.google.com/home/usersettings>. Treat the refresh token inside `CLASPRC_JSON` as a high-value credential and rotate it if exposed.

The web-app deployment ID is intentionally pinned in the workflow:

```text
AKfycbzIMOO0eQ12WjgRvLmYAdn3zryB57Ush6uWfQWc-iHNvVu6X0ULbPfPv7WMaYdMp2Tq
```

A manual workflow run with operation `pull-artifact` executes `clasp pull` into an isolated directory and uploads the remote project snapshot as a seven-day GitHub Actions artifact. It never commits pulled code automatically and strips `.clasp.json` before upload.

After the secrets are configured, avoid editing source directly in the Apps Script editor. Repository changes should flow through pull requests and `main`; manual remote edits can be inspected through `pull-artifact`.

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

Adding scopes can cause Google to request additional authorization approval. A clasp redeployment updates the existing web-app deployment automatically.

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
