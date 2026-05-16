// Storage adapter selector. Reads env to decide which adapter is active
// and exposes a `publish()` helper that the agent task uses after writing
// files to the per-task outputs/ directory.
//
// Convention: an agent run that wants to publish files writes them to
//   ${OUTPUTS_DIR}/<taskId>/...
// The dev-server's runTask scans that directory after `claude` exits and
// publishes each file via the configured adapter.

import { makeLocalAdapter } from './local.js';
import { makeS3R2Adapter } from './s3-r2.js';
import { makeGcsAdapter } from './gcs.js';
import { makeDriveAdapter } from './drive.js';
import type {
  PublishOptions,
  PublishedArtifact,
  StorageAdapter,
  StorageProvider,
} from './types.js';

export type { PublishOptions, PublishedArtifact, StorageAdapter, StorageProvider };

function getEnv(name: string, fallback?: string): string | undefined {
  const v = process.env[name];
  if (v && v.length > 0) return v;
  return fallback;
}

function getRequiredEnv(name: string): string {
  const v = process.env[name];
  if (!v || v.length === 0) {
    throw new Error(`storage adapter requires env var ${name}`);
  }
  return v;
}

let cachedDefault: StorageAdapter | null = null;

export function getDefaultAdapter(): StorageAdapter {
  if (cachedDefault) return cachedDefault;
  const provider = (getEnv('DEFAULT_STORAGE_PROVIDER', 'local') ??
    'local') as StorageProvider;
  cachedDefault = getAdapter(provider);
  return cachedDefault;
}

export function getAdapter(provider: StorageProvider): StorageAdapter {
  switch (provider) {
    case 'local':
      return makeLocalAdapter({
        rootDir: getEnv('LOCAL_STORAGE_ROOT', '/home/agent/workspace/published')!,
        publicBaseUrl: getRequiredEnv('LOCAL_STORAGE_PUBLIC_BASE_URL'),
      });
    case 's3':
      return makeS3R2Adapter({
        provider: 's3',
        bucket: getRequiredEnv('S3_BUCKET'),
        publicBaseUrl: getRequiredEnv('S3_PUBLIC_BASE_URL'),
        region: getEnv('S3_REGION', 'us-east-1'),
        accessKeyId: getEnv('S3_ACCESS_KEY_ID'),
        secretAccessKey: getEnv('S3_SECRET_ACCESS_KEY'),
      });
    case 'r2':
      return makeS3R2Adapter({
        provider: 'r2',
        bucket: getRequiredEnv('R2_BUCKET'),
        endpoint: getRequiredEnv('R2_ENDPOINT'),
        publicBaseUrl: getRequiredEnv('R2_PUBLIC_BASE_URL'),
        region: getEnv('R2_REGION', 'auto'),
        accessKeyId: getEnv('R2_ACCESS_KEY_ID'),
        secretAccessKey: getEnv('R2_SECRET_ACCESS_KEY'),
      });
    case 'gcs':
      return makeGcsAdapter({
        projectId: getRequiredEnv('GCS_PROJECT_ID'),
        bucket: getRequiredEnv('GCS_BUCKET'),
        keyJsonBase64: getEnv('GCS_KEY_JSON_BASE64'),
        publicBaseUrl: getRequiredEnv('GCS_PUBLIC_BASE_URL'),
      });
    case 'drive':
      return makeDriveAdapter({
        folderId: getRequiredEnv('DRIVE_FOLDER_ID'),
        keyJsonBase64: getRequiredEnv('DRIVE_KEY_JSON_BASE64'),
        shareMode: (getEnv('DRIVE_SHARE_MODE', 'anyone') as
          | 'anyone'
          | 'domain'
          | 'private'),
      });
  }
}

/**
 * Convenience: publish via the default adapter (or `provider` override).
 */
export async function publishArtifact(
  opts: PublishOptions & { provider?: StorageProvider },
): Promise<PublishedArtifact> {
  const adapter = opts.provider ? getAdapter(opts.provider) : getDefaultAdapter();
  return adapter.publish(opts);
}
