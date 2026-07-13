import { openDB, type DBSchema, type IDBPDatabase } from "idb";
import type {
  DraftNoteKind,
  LocalDraftNoteRecord,
  OutboxMutation,
  OutboxStatus,
  RecordSyncState,
  SyncConflict,
} from "./types";

export const DEFAULT_SYNC_DB_NAME = "canonical-sync-v1";
const SYNC_DB_VERSION = 1;

export interface MetaEntry {
  accountKey: string;
  key: "clientId" | "nextLocalSeq";
  value: string | number;
}

export interface AccountState {
  accountKey: string;
  pullCursor: string | null;
  lastSuccessfulSyncAt: number | null;
  lastError: string | null;
}

export interface CanonicalSyncDb extends DBSchema {
  meta: {
    key: [string, MetaEntry["key"]];
    value: MetaEntry;
  };
  accountState: {
    key: string;
    value: AccountState;
  };
  records: {
    key: [string, DraftNoteKind, string];
    value: LocalDraftNoteRecord;
    indexes: {
      byAccountKind: [string, DraftNoteKind];
      byAccountState: [string, RecordSyncState];
    };
  };
  outbox: {
    key: [string, string];
    value: OutboxMutation;
    indexes: {
      byAccountStatusNextAttempt: [string, OutboxStatus, number];
      byAccountEntityLocalSeq: [string, DraftNoteKind, string, number];
    };
  };
  conflicts: {
    key: [string, DraftNoteKind, string];
    value: SyncConflict;
    indexes: {
      byAccount: string;
    };
  };
}

export type SyncDatabase = IDBPDatabase<CanonicalSyncDb>;

export async function openSyncDatabase(name = DEFAULT_SYNC_DB_NAME): Promise<SyncDatabase> {
  return openDB<CanonicalSyncDb>(name, SYNC_DB_VERSION, {
    upgrade(database) {
      const meta = database.createObjectStore("meta", {
        keyPath: ["accountKey", "key"],
      });
      void meta;

      database.createObjectStore("accountState", { keyPath: "accountKey" });

      const records = database.createObjectStore("records", {
        keyPath: ["accountKey", "kind", "id"],
      });
      records.createIndex("byAccountKind", ["accountKey", "kind"]);
      records.createIndex("byAccountState", ["accountKey", "state"]);

      const outbox = database.createObjectStore("outbox", {
        keyPath: ["accountKey", "mutationId"],
      });
      outbox.createIndex("byAccountStatusNextAttempt", ["accountKey", "status", "nextAttemptAt"]);
      outbox.createIndex("byAccountEntityLocalSeq", [
        "accountKey",
        "key.kind",
        "key.id",
        "localSeq",
      ]);

      const conflicts = database.createObjectStore("conflicts", {
        keyPath: ["accountKey", "kind", "id"],
      });
      conflicts.createIndex("byAccount", "accountKey");
    },
  });
}
