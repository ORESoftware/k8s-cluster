// Google Drive adapter — stubbed.
//
// To activate, install `googleapis` and wire the upload below. Drive is
// folder-based rather than bucket+key, so we treat DRIVE_FOLDER_ID as
// the parent and upload each file directly into it.
//
// Env contract:
//   DRIVE_FOLDER_ID
//   DRIVE_KEY_JSON_BASE64   (base64-encoded service account JSON; the
//                            service account must have Editor on the folder)
//   DRIVE_SHARE_MODE         'anyone' | 'domain' | 'private'  (default
//                            'anyone' so the URL we emit is fetchable)
import { basename } from 'node:path';
export function makeDriveAdapter(opts) {
    return {
        provider: 'drive',
        async publish(p) {
            const filename = p.filename ?? basename(p.filePath);
            // -------------------------------------------------------------
            //  TODO(remote-dev): real Drive upload.
            //  import { google } from 'googleapis';
            //  const credentials = JSON.parse(
            //    Buffer.from(opts.keyJsonBase64, 'base64').toString('utf8'),
            //  );
            //  const auth = new google.auth.JWT({
            //    email: credentials.client_email,
            //    key: credentials.private_key,
            //    scopes: ['https://www.googleapis.com/auth/drive'],
            //  });
            //  const drive = google.drive({ version: 'v3', auth });
            //  const created = await drive.files.create({
            //    requestBody: {
            //      name: filename,
            //      parents: [opts.folderId],
            //      mimeType: p.contentType,
            //    },
            //    media: {
            //      mimeType: p.contentType,
            //      body: fs.createReadStream(p.filePath),
            //    },
            //    fields: 'id, webViewLink, webContentLink',
            //  });
            //  if ((opts.shareMode ?? 'anyone') === 'anyone') {
            //    await drive.permissions.create({
            //      fileId: created.data.id!,
            //      requestBody: { role: 'reader', type: 'anyone' },
            //    });
            //  }
            //  return {
            //    filename,
            //    storageProvider: 'drive',
            //    storageKey: created.data.id ?? undefined,
            //    url: created.data.webViewLink ?? created.data.webContentLink ?? '',
            //  };
            // -------------------------------------------------------------
            throw new Error('Drive adapter is scaffolded but the SDK call is not wired yet.\n' +
                'Install googleapis and replace the TODO block in drive.ts.');
        },
    };
}
//# sourceMappingURL=drive.js.map