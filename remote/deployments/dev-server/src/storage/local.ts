// Local storage adapter — for dev / testing only. Copies the file into a
// directory the container exposes via static-file serving (or a mounted
// volume). Produces a URL relative to LOCAL_STORAGE_PUBLIC_BASE_URL.
//
// Useful when you want to validate the artifact-event flow end-to-end
// without provisioning a real cloud bucket.

import {
  copyFile,
  mkdir,
  stat,
  open,
} from 'node:fs/promises';
import { basename, dirname, join } from 'node:path';
import { createHash } from 'node:crypto';

import type {
  PublishOptions,
  PublishedArtifact,
  StorageAdapter,
} from './types.js';

export interface LocalAdapterOptions {
  /** Filesystem dir the container writes published files into. */
  rootDir: string;
  /** Base URL the browser uses to fetch from rootDir (must be exposed). */
  publicBaseUrl: string;
}

export function makeLocalAdapter(opts: LocalAdapterOptions): StorageAdapter {
  return {
    provider: 'local',
    async publish(p: PublishOptions): Promise<PublishedArtifact> {
      const filename = p.filename ?? basename(p.filePath);
      const key = p.destinationKey ?? `remote-dev/${p.taskId}/${filename}`;
      const dest = join(opts.rootDir, key);
      await mkdir(dirname(dest), { recursive: true });
      await copyFile(p.filePath, dest);

      const st = await stat(dest);
      const sha256 = await sha256File(dest);

      return {
        filename,
        contentType: p.contentType,
        sizeBytes: st.size,
        storageProvider: 'local',
        storageKey: key,
        url: `${opts.publicBaseUrl.replace(/\/+$/, '')}/${key}`,
        sha256,
      };
    },
  };
}

async function sha256File(path: string): Promise<string> {
  const fh = await open(path, 'r');
  try {
    const hash = createHash('sha256');
    const buf = Buffer.alloc(64 * 1024);
    let n: number;
    do {
      const { bytesRead } = await fh.read(buf, 0, buf.length, null);
      n = bytesRead;
      if (n > 0) hash.update(buf.subarray(0, n));
    } while (n > 0);
    return hash.digest('hex');
  } finally {
    await fh.close();
  }
}
