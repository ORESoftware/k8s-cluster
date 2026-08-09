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
- The canonical repository relay uses authenticated GET because the Apps Script response redirect terminates at a GET-only `googleusercontent.com` endpoint. Forcing POST across the 302/303 redirect returns HTTP 405.
- Relay protocol 2 accepts exactly one ciphertext per published run ID. A second matching submission fails closed instead of choosing a last writer.
- The decrypted handoff must contain the published `run_id`, bridge token, and a distinct high-entropy archive passphrase.
- Completion metadata binds the input ciphertext, plaintext export manifest, and encrypted archive by SHA-256.
- Plaintext `SHA256SUMS` excludes itself; message export files are never committed or printed to workflow logs.
- GET query tokens can appear in infrastructure URL logs, so rotate the bridge token after an import.
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
7. Run `setupBridge()` manually and approve access. Copy `CHAT_BRIDGE_TOKEN` from the execution log into an approved secret manager; never place it in Git, issues, PRs, Linear, Slack, or Chat.
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

The web-app deployment ID is intentionally pinned in the synchronization workflow. A manual workflow run with operation `pull-artifact` executes `clasp pull` into an isolated directory and uploads the remote project snapshot as a seven-day GitHub Actions artifact. It never commits pulled code automatically and strips `.clasp.json` before upload.

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

Authenticated GET:

```text
GET <EXEC_URL>?action=messages&token=<TOKEN>&pageSize=100
```

Use the returned `data.nextPageToken` as `pageToken` on the next request. Supported authenticated actions are `status`, `probe`, `space`, and `messages`.

The old automatic POST relay forced POST across Apps Script's 302/303 response redirect and therefore reached a GET-only echo endpoint with HTTP 405. `.github/workflows/ephemeral-google-chat-relay.yml` is now a manual retirement notice and performs no network call. Use `.github/workflows/ephemeral-google-chat-relay-get.yml` for encrypted imports.

A direct client may still send the original request as POST only when it lets the redirect change to GET. Never use `--post302` or `--post303`, and never put a credential in process arguments or committed configuration.

After the import, run `rotateBridgeToken()` or `disableBridge()` from the editor.

## Ephemeral encrypted relay protocol 2

1. The GET relay publishes a one-time 3072-bit RSA public key and run ID on the fixed relay issue.
2. The submitter generates a unique archive passphrase and encrypts one compact JSON payload using RSA-OAEP SHA-256:

   ```json
   {
     "run_id": "<PUBLISHED_RUN_ID>",
     "token": "<BRIDGE_TOKEN>",
     "archive_passphrase": "<UNIQUE_HIGH_ENTROPY_PASSPHRASE>"
   }
   ```

3. Submit exactly one `CHAT_RELAY_GET_CIPHERTEXT` comment for that run. Duplicate matching comments reject the run after a bounded race-detection window.
4. The workflow verifies the payload run ID, fixed space, fixed display name, and earliest boundary; paginates the GET API; writes a non-self-referential checksum manifest; encrypts the archive; and retains it for one day.
5. The completion comment includes the ciphertext, export-manifest, and encrypted-archive SHA-256 values. Verify all three before accepting the audit evidence.
6. Destroy the local passphrase/export and rotate or disable the bridge token after reconciliation.

[`test_relay_workflows.py`](./test_relay_workflows.py) and [`.github/workflows/google-chat-relay-contract.yml`](../../.github/workflows/google-chat-relay-contract.yml) enforce the non-negotiable workflow invariants.

## Bulk page fetch

[`fetch-bridge-pages.mjs`](./fetch-bridge-pages.mjs) pages through the HTTP bridge and writes raw page files that the planner reads directly.

```bash
CHAT_BRIDGE_TOKEN=<token> node tools/google-chat-space-export/fetch-bridge-pages.mjs \
  --out ./private/google-chat-export
```

The token comes from the environment so it never lands in argv or a query string. The script does not filter by date: the bridge floor is fixed and callers cannot widen it, so windowing belongs to the planner's `--since`.

The fetcher sends POST only to the original Apps Script `/exec` URL and allows the 302/303 redirect to become GET. Re-posting to the `googleusercontent.com` target answers HTTP 405 with an HTML body.

## Dry-run import planner

[`import-plan.mjs`](./import-plan.mjs) is a read-only gate between the raw Chat export and Linear. It accepts both HTTP bridge pages (`{ok,data.messages}`) and Gmail export attachments (`{messages,...}`), validates the fixed space/date boundary, deduplicates repeated pages, groups messages conservatively by thread, and emits deterministic JSON and Markdown reports.

```bash
node tools/google-chat-space-export/import-plan.mjs \
  --input ./private/google-chat-export \
  --since 2026-06-05T04:00:00.000Z \
  --existing-index ./private/linear-issue-index.json \
  --project-map tools/google-chat-space-export/import-project-map.example.json \
  --json ./private/google-chat-import-plan.json \
  --markdown ./private/google-chat-import-plan.md
```

`--since` narrows the plan to messages created at or after the given instant, for reconciling one stretch of history at a time. It can only narrow: a value earlier than the fixed `2026-05-10T04:00:00.000Z` boundary is rejected rather than silently clamped, so the command line can never widen access to earlier history. Deduplication still runs across every supplied message, so `uniqueMessages` and `duplicateMessages` continue to describe the whole input while `plannedMessages` and `windowedOutMessages` describe the window. A narrowed window is stated in the Markdown report and recorded as `source.windowStartInclusive` in the JSON plan.

The optional existing-issue index can be an array or `{ "issues": [...] }`. Each issue may contain `id`, `identifier`, `title`, `description`, `comments`, `project`, `state`, `url`, and explicit `sourceKeys`. The planner also discovers deterministic `google-chat:...` keys embedded in descriptions or comments.

The project map has `repositories` and `organizations` objects. Explicit repository mappings outrank organization mappings. Unmapped or conflicting references are left for manual review rather than silently routed to a catch-all project.

Candidate actions are:

- `create` — substantive, high-confidence work with no detected duplicate;
- `comment-existing` — one or more exact Chat source keys already belong to an issue;
- `manual-review` — title duplicate, ambiguous project, or an unusually large thread;
- `skip-non-actionable` — acknowledgements or empty/deleted-only threads.

The planner never uses a Linear API key and never writes to Linear. Run the generated plan through human/agent review, then perform the controlled apply phase in small batches. A second dry run after applying must propose zero duplicate creations.

## Linear import rules

Group messages by thread and work item, search completed and archived Linear issues, persist each deterministic `sourceKey`, and add context to existing issues instead of creating duplicates. Tracking issue: `DEN-266` in `github.com/ORESoftware`.
