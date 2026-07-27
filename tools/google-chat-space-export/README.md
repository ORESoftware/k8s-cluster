# Google Chat space export helper

This is an account-side Google Apps Script fallback for exporting the Google Chat space:

- display name: `alex-alex-me`
- space resource: `spaces/AAQAoHKdzvI`
- source: <https://chat.google.com/room/AAQAoHKdzvI?cls=5>
- start date: **May 10, 2026 at 00:00 America/New_York**

It does not scrape the Chat web UI. It authenticates as the Google user running the script, calls the read-only Google Chat API, and writes private JSON files into that user's Drive.

## Security properties

- Requests only `chat.messages.readonly` for Google Chat.
- Never logs message text, OAuth tokens, or refresh tokens.
- Stores pagination state without message content in Script Properties.
- Writes export files to a newly created private Drive folder.
- Does not call Linear or any third-party endpoint.
- Does not download attachment binaries; it preserves attachment metadata only.
- Uses deterministic source keys such as `google-chat:AAQAoHKdzvI:<message-resource-name>` for later Linear deduplication.

The full Drive scope is required because Apps Script's `DriveApp.createFolder`, `DriveApp.getFolderById`, and folder file-creation methods require it. The script does not enumerate unrelated Drive files.

## Run it in Google Apps Script

1. Sign into the Google account that is a member of `alex-alex-me`.
2. Open <https://script.new> and create a standalone Apps Script project.
3. Replace `Code.gs` with [`Code.gs`](./Code.gs).
4. In **Project Settings**, enable **Show `appsscript.json` manifest file in editor**.
5. Replace the manifest with [`appsscript.json`](./appsscript.json).
6. Link the script to a Google Cloud project in which the **Google Chat API** is enabled.
7. In **Services**, confirm that the advanced **Google Chat API v1** service is enabled as `Chat`.
8. Select `startGoogleChatExport` and click **Run**.
9. Approve the requested scopes using the account that can read the space.
10. Check the execution log for the private Drive folder URL. For long histories, the script creates a time-based continuation trigger and writes one API page per file.
11. Run `getGoogleChatExportStatus` at any time to see counts and the folder URL without returning message contents.

The completed folder contains:

- `export-config.json`
- `messages-part-00001.json`, `messages-part-00002.json`, ...
- `export-summary.json`

If an API call fails, the folder also receives `export-error.json` with non-secret troubleshooting information.

## Account compatibility

Google's current `spaces.messages.list` setup guide documents a Business or Enterprise Google Workspace account as a prerequisite. If the authenticated account is a personal `@gmail.com` account and the Chat API rejects the request, use Google Takeout instead. Google says a Chat export can include memberships, messages, and attachments for direct messages, group messages, and spaces, subject to restrictions for spaces created by work or school accounts.

## Import into Linear

Do not create one Linear issue per message. The importer should:

1. Group messages by thread and semantic work item.
2. Search all existing Linear issues, including completed and archived issues.
3. Check deterministic message and thread source keys before creating anything.
4. Route explicit GitHub organization/repository matches first, then use semantic project matching.
5. Add subsequent context to an existing issue as comments when it represents the same work.
6. Keep the original space, message, thread, timestamp, and author provenance.

The tracking issue is `DEN-266` in the Linear project `github.com/ORESoftware`.

## Official references

- Google Chat API: list messages: <https://developers.google.com/workspace/chat/list-messages>
- `spaces.messages.list` reference: <https://developers.google.com/workspace/chat/api/reference/rest/v1/spaces.messages/list>
- Google Chat data export: <https://support.google.com/chat/answer/10126829>
- Apps Script Drive authorization: <https://developers.google.com/apps-script/reference/drive/drive-app>
