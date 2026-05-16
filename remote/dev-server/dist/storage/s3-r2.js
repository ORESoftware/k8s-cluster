// S3 / R2 adapter — R2 is S3-compatible, so one client serves both. The
// difference is just `endpoint` (and `region: "auto"` for R2).
//
// Env contract (per provider):
//   R2:  R2_REGION=auto, R2_ENDPOINT=https://<account>.r2.cloudflarestorage.com,
//        R2_BUCKET, R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY, R2_PUBLIC_BASE_URL
//   S3:  S3_REGION=us-east-1, (no endpoint), S3_BUCKET, S3_ACCESS_KEY_ID,
//        S3_SECRET_ACCESS_KEY, S3_PUBLIC_BASE_URL
//
// The dev-server's runTask scans `${OUTPUTS_DIR}/<taskId>/` after each agent
// run and calls `publish()` once per file, emitting an `artifact` event with
// the resulting URL.
import { basename } from 'node:path';
import { stat, open } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { S3Client, PutObjectCommand } from '@aws-sdk/client-s3';
export function makeS3R2Adapter(opts) {
    const provider = opts.provider;
    // R2 reports `region: auto`. AWS S3 requires a real region. Default
    // accordingly when the caller didn't pass one.
    const region = opts.region ?? (opts.endpoint ? 'auto' : 'us-east-1');
    const client = new S3Client({
        region,
        endpoint: opts.endpoint,
        credentials: opts.accessKeyId && opts.secretAccessKey
            ? {
                accessKeyId: opts.accessKeyId,
                secretAccessKey: opts.secretAccessKey,
            }
            : undefined,
        // R2 / MinIO require path-style addressing. Real AWS S3 supports
        // virtual-host style (the default) so we only flip when an
        // explicit endpoint is configured.
        forcePathStyle: !!opts.endpoint,
    });
    return {
        provider,
        async publish(p) {
            const filename = p.filename ?? basename(p.filePath);
            const key = p.destinationKey ?? `remote-dev/${p.taskId}/${filename}`;
            // eslint-disable-next-line security/detect-non-literal-fs-filename -- caller-supplied upload path is intentional
            const st = await stat(p.filePath);
            const sha256 = await sha256File(p.filePath);
            await client.send(new PutObjectCommand({
                Bucket: opts.bucket,
                Key: key,
                // eslint-disable-next-line security/detect-non-literal-fs-filename -- caller-supplied upload path is intentional
                Body: createReadStream(p.filePath),
                ContentType: p.contentType,
                ContentLength: st.size,
            }));
            return {
                filename,
                contentType: p.contentType,
                sizeBytes: st.size,
                storageProvider: provider,
                storageBucket: opts.bucket,
                storageKey: key,
                url: `${opts.publicBaseUrl.replace(/\/+$/, '')}/${key}`,
                sha256,
            };
        },
    };
}
async function sha256File(path) {
    // eslint-disable-next-line security/detect-non-literal-fs-filename -- caller-supplied path for hashing is intentional
    const fh = await open(path, 'r');
    try {
        const hash = createHash('sha256');
        const buf = Buffer.alloc(64 * 1024);
        let n;
        do {
            const { bytesRead } = await fh.read(buf, 0, buf.length, null);
            n = bytesRead;
            if (n > 0) {
                hash.update(buf.subarray(0, n));
            }
        } while (n > 0);
        return hash.digest('hex');
    }
    finally {
        await fh.close();
    }
}
//# sourceMappingURL=s3-r2.js.map