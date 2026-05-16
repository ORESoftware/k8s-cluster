// Google Cloud Storage adapter — stubbed.
//
// To activate, install `@google-cloud/storage` and wire the upload below.
// Env contract:
//   GCS_PROJECT_ID
//   GCS_BUCKET
//   GCS_KEY_JSON_BASE64   (base64-encoded JSON service account key)
//   GCS_PUBLIC_BASE_URL   (e.g. https://storage.googleapis.com/<bucket> for
//                          public buckets, or a CDN front)
import { basename } from 'node:path';
export function makeGcsAdapter(opts) {
    return {
        provider: 'gcs',
        async publish(p) {
            const filename = p.filename ?? basename(p.filePath);
            const key = p.destinationKey ?? `remote-dev/${p.taskId}/${filename}`;
            // -------------------------------------------------------------
            //  TODO(remote-dev): real GCS upload.
            //  import { Storage } from '@google-cloud/storage';
            //  const credentials = opts.keyJsonBase64
            //    ? JSON.parse(Buffer.from(opts.keyJsonBase64, 'base64').toString('utf8'))
            //    : undefined;
            //  const storage = new Storage({ projectId: opts.projectId, credentials });
            //  await storage.bucket(opts.bucket).upload(p.filePath, {
            //    destination: key,
            //    metadata: p.contentType ? { contentType: p.contentType } : undefined,
            //  });
            // -------------------------------------------------------------
            throw new Error('GCS adapter is scaffolded but the SDK call is not wired yet.\n' +
                'Install @google-cloud/storage and replace the TODO block in gcs.ts.');
            // return {
            //   filename,
            //   contentType: p.contentType,
            //   storageProvider: 'gcs',
            //   storageBucket: opts.bucket,
            //   storageKey: key,
            //   url: `${opts.publicBaseUrl.replace(/\/+$/, '')}/${key}`,
            // };
        },
    };
}
//# sourceMappingURL=gcs.js.map