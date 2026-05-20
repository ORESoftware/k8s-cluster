// Storage adapter shape — every provider implementation conforms.

export type StorageProvider = 's3' | 'r2' | 'gcs' | 'drive' | 'local';

export interface PublishedArtifact {
  filename: string;
  contentType?: string;
  sizeBytes?: number;
  storageProvider: StorageProvider;
  storageBucket?: string;
  storageKey?: string;
  url: string;
  signedUrlExpiresAt?: string; // ISO
  sha256?: string;
  meta?: Record<string, unknown>;
}

export interface PublishOptions {
  taskId: string;
  filePath: string;
  filename?: string; // defaults to basename(filePath)
  contentType?: string; // sniffed if absent
  // Per-call override of the destination key/path inside the storage bucket.
  // Defaults to `remote-dev/<taskId>/<filename>`.
  destinationKey?: string;
}

export interface StorageAdapter {
  readonly provider: StorageProvider;
  publish(opts: PublishOptions): Promise<PublishedArtifact>;
}
