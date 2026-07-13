import type { IDBPTransaction } from "idb";
import {
  type AccountState,
  type CanonicalSyncDb,
  openSyncDatabase,
  type SyncDatabase,
} from "./db";
import { randomId } from "./id";
import {
  DRAFT_NOTE_SCHEMA_VERSION,
  type ChangesResponse,
  type ConfirmedShadow,
  type DraftNoteKey,
  type DraftNoteValue,
  type EffectiveDraftNote,
  type LocalDraftNoteRecord,
  type MutationOperation,
  type MutationResult,
  type OptimisticShadow,
  type OutboxMutation,
  type SyncConflict,
  type WireRecord,
} from "./types";
import {
  assertAccountKey,
  assertDecimalVersion,
  assertDraftNoteValue,
  assertRecordId,
  assertWireRecord,
  compareDecimalVersions,
} from "./validation";

const LOCAL_SEQUENCE_META_KEY = "nextLocalSeq" as const;
const CLIENT_ID_META_KEY = "clientId" as const;
const MAX_LOCAL_SEQUENCE = Number.MAX_SAFE_INTEGER - 1;

type WriteTransaction = IDBPTransaction<
  CanonicalSyncDb,
  ("meta" | "accountState" | "records" | "outbox" | "conflicts")[],
  "readwrite"
>;

async function clearAccountData(database: SyncDatabase, accountKey: string): Promise<void> {
  const transaction = database.transaction(
    ["meta", "accountState", "records", "outbox", "conflicts"],
    "readwrite",
  );
  const ranges = {
    meta: IDBKeyRange.bound([accountKey, ""], [accountKey, "\uffff"]),
    records: IDBKeyRange.bound([accountKey, "", ""], [accountKey, "\uffff", "\uffff"]),
    outbox: IDBKeyRange.bound([accountKey, ""], [accountKey, "\uffff"]),
    conflicts: IDBKeyRange.bound([accountKey, "", ""], [accountKey, "\uffff", "\uffff"]),
  };
  for (const key of await transaction.objectStore("meta").getAllKeys(ranges.meta)) {
    await transaction.objectStore("meta").delete(key);
  }
  await transaction.objectStore("accountState").delete(accountKey);
  for (const key of await transaction.objectStore("records").getAllKeys(ranges.records)) {
    await transaction.objectStore("records").delete(key);
  }
  for (const key of await transaction.objectStore("outbox").getAllKeys(ranges.outbox)) {
    await transaction.objectStore("outbox").delete(key);
  }
  for (const key of await transaction.objectStore("conflicts").getAllKeys(ranges.conflicts)) {
    await transaction.objectStore("conflicts").delete(key);
  }
  await transaction.done;
}

function recordKey(accountKey: string, id: string): [string, "draft_note", string] {
  return [accountKey, "draft_note", id];
}

function outboxKey(accountKey: string, mutationId: string): [string, string] {
  return [accountKey, mutationId];
}

function entityOutboxRange(accountKey: string, id: string): IDBKeyRange {
  return IDBKeyRange.bound(
    [accountKey, "draft_note", id, 0],
    [accountKey, "draft_note", id, Number.MAX_SAFE_INTEGER],
  );
}

function toConfirmed(record: WireRecord): ConfirmedShadow {
  const confirmed: ConfirmedShadow = {
    version: record.version,
    schemaVersion: DRAFT_NOTE_SCHEMA_VERSION,
    deleted: record.deleted,
  };
  if (record.value !== undefined) {
    confirmed.value = structuredClone(record.value);
  }
  return confirmed;
}

function operationFromOutbox(operation: OutboxMutation): MutationOperation {
  const result: MutationOperation = {
    mutationId: operation.mutationId,
    key: operation.key,
    action: operation.action,
    baseVersion: operation.baseVersion,
    schemaVersion: operation.schemaVersion,
  };
  if (operation.value !== undefined) {
    result.value = operation.value;
  }
  return result;
}

async function nextLocalSequence(transaction: WriteTransaction, accountKey: string): Promise<number> {
  const meta = transaction.objectStore("meta");
  const entry = await meta.get([accountKey, LOCAL_SEQUENCE_META_KEY]);
  const current = typeof entry?.value === "number" ? entry.value : 1;
  if (!Number.isSafeInteger(current) || current < 1 || current > MAX_LOCAL_SEQUENCE) {
    throw new Error("local mutation sequence is exhausted or corrupt");
  }
  await meta.put({ accountKey, key: LOCAL_SEQUENCE_META_KEY, value: current + 1 });
  return current;
}

export interface OpenSyncStoreOptions {
  accountKey: string;
  databaseName?: string;
  now?: () => number;
}

export interface PushApplySummary {
  appliedMutationIds: string[];
  conflictedMutationIds: string[];
  failedMutationIds: string[];
}

export class SyncStore {
  readonly accountKey: string;
  readonly clientId: string;
  readonly database: SyncDatabase;
  private readonly now: () => number;

  private constructor(database: SyncDatabase, accountKey: string, clientId: string, now: () => number) {
    this.database = database;
    this.accountKey = accountKey;
    this.clientId = clientId;
    this.now = now;
  }

  static async open(options: OpenSyncStoreOptions): Promise<SyncStore> {
    assertAccountKey(options.accountKey);
    const database = await openSyncDatabase(options.databaseName);
    const otherAccounts = (await database.getAllKeys("accountState")).filter(
      (accountKey) => accountKey !== options.accountKey,
    );
    for (const accountKey of otherAccounts) {
      await clearAccountData(database, accountKey);
    }
    const transaction = database.transaction(["meta", "accountState"], "readwrite");
    const meta = transaction.objectStore("meta");
    const existingClientId = await meta.get([options.accountKey, CLIENT_ID_META_KEY]);
    const clientId =
      typeof existingClientId?.value === "string" ? existingClientId.value : randomId();

    if (existingClientId === undefined) {
      await meta.put({ accountKey: options.accountKey, key: CLIENT_ID_META_KEY, value: clientId });
    }
    const sequence = await meta.get([options.accountKey, LOCAL_SEQUENCE_META_KEY]);
    if (sequence === undefined) {
      await meta.put({ accountKey: options.accountKey, key: LOCAL_SEQUENCE_META_KEY, value: 1 });
    }

    const states = transaction.objectStore("accountState");
    if ((await states.get(options.accountKey)) === undefined) {
      await states.put({
        accountKey: options.accountKey,
        pullCursor: null,
        lastSuccessfulSyncAt: null,
        lastError: null,
      });
    }
    await transaction.done;
    return new SyncStore(database, options.accountKey, clientId, options.now ?? Date.now);
  }

  close(): void {
    this.database.close();
  }

  async clearAccount(): Promise<void> {
    await clearAccountData(this.database, this.accountKey);
  }

  async putDraftNote(id: string, value: DraftNoteValue): Promise<OutboxMutation> {
    assertRecordId(id);
    assertDraftNoteValue(value);
    return this.queueMutation({ kind: "draft_note", id }, "put", value);
  }

  async deleteDraftNote(id: string): Promise<OutboxMutation> {
    assertRecordId(id);
    return this.queueMutation({ kind: "draft_note", id }, "delete");
  }

  private async queueMutation(
    key: DraftNoteKey,
    action: "put" | "delete",
    value?: DraftNoteValue,
  ): Promise<OutboxMutation> {
    const transaction = this.database.transaction(
      ["meta", "accountState", "records", "outbox", "conflicts"],
      "readwrite",
    );
    const conflict = await transaction.objectStore("conflicts").get(recordKey(this.accountKey, key.id));
    if (conflict !== undefined) {
      transaction.abort();
      throw new Error("record has an unresolved sync conflict");
    }

    const records = transaction.objectStore("records");
    const existing = await records.get(recordKey(this.accountKey, key.id));
    if (existing?.state === "failed") {
      transaction.abort();
      throw new Error("record has a failed mutation that must be resolved first");
    }

    const localSeq = await nextLocalSequence(transaction, this.accountKey);
    const outbox = transaction.objectStore("outbox");
    const entityOperations = await outbox
      .index("byAccountEntityLocalSeq")
      .getAll(entityOutboxRange(this.accountKey, key.id));
    entityOperations.sort((left, right) => left.localSeq - right.localSeq);

    const replaceable = entityOperations.filter(
      (operation) => operation.status === "queued" && operation.attempts === 0,
    );
    const immutable = entityOperations.filter(
      (operation) => operation.status === "inflight" || operation.attempts > 0,
    );
    for (const operation of replaceable) {
      await outbox.delete(outboxKey(this.accountKey, operation.mutationId));
    }

    const newestImmutable = immutable.at(-1);
    const oldestReplaceable = replaceable.at(0);
    const baseVersion = oldestReplaceable?.baseVersion ?? existing?.confirmed?.version ?? null;
    if (baseVersion !== null) {
      assertDecimalVersion(baseVersion);
    }
    const dependsOnMutationId =
      newestImmutable?.mutationId ?? oldestReplaceable?.dependsOnMutationId ?? null;
    const mutationId = randomId();
    const now = this.now();
    const mutation: OutboxMutation = {
      accountKey: this.accountKey,
      mutationId,
      key,
      action,
      baseVersion,
      schemaVersion: DRAFT_NOTE_SCHEMA_VERSION,
      localSeq,
      status: "queued",
      dependsOnMutationId,
      attempts: 0,
      nextAttemptAt: now,
      leaseOwner: null,
      leaseExpiresAt: null,
      createdAt: now,
      lastError: null,
    };
    if (value !== undefined) {
      mutation.value = structuredClone(value);
    }
    await outbox.put(mutation);

    const optimistic: OptimisticShadow = { localSeq, action };
    if (value !== undefined) {
      optimistic.value = structuredClone(value);
    }
    const record: LocalDraftNoteRecord = {
      accountKey: this.accountKey,
      kind: "draft_note",
      id: key.id,
      confirmed: existing?.confirmed ?? null,
      optimistic,
      state: "pending",
      updatedAt: now,
    };
    await records.put(record);
    await transaction.done;
    return mutation;
  }

  async getRecord(id: string): Promise<LocalDraftNoteRecord | undefined> {
    return this.database.get("records", recordKey(this.accountKey, id));
  }

  async getConflict(id: string): Promise<SyncConflict | undefined> {
    return this.database.get("conflicts", recordKey(this.accountKey, id));
  }

  async getAccountState(): Promise<AccountState> {
    const state = await this.database.get("accountState", this.accountKey);
    if (state === undefined) {
      throw new Error("account state is missing");
    }
    return state;
  }

  async listOutbox(): Promise<OutboxMutation[]> {
    const operations = await this.database.getAllFromIndex(
      "outbox",
      "byAccountEntityLocalSeq",
      IDBKeyRange.bound(
        [this.accountKey, "draft_note", "", 0],
        [this.accountKey, "draft_note", "\uffff", Number.MAX_SAFE_INTEGER],
      ),
    );
    return operations.sort((left, right) => left.localSeq - right.localSeq);
  }

  async listEffectiveDraftNotes(): Promise<EffectiveDraftNote[]> {
    const records = await this.database.getAllFromIndex("records", "byAccountKind", [
      this.accountKey,
      "draft_note",
    ]);
    return records.flatMap((record): EffectiveDraftNote[] => {
      if (record.state === "conflict") {
        return [];
      }
      if (record.optimistic !== null) {
        if (record.optimistic.action === "delete" || record.optimistic.value === undefined) {
          return [];
        }
        return [
          {
            id: record.id,
            value: structuredClone(record.optimistic.value),
            version: record.confirmed?.version ?? null,
            pending: record.state === "pending",
            conflicted: false,
            failed: record.state === "failed",
          },
        ];
      }
      if (record.confirmed === null || record.confirmed.deleted || record.confirmed.value === undefined) {
        return [];
      }
      return [
        {
          id: record.id,
          value: structuredClone(record.confirmed.value),
          version: record.confirmed.version,
          pending: false,
          conflicted: false,
          failed: false,
        },
      ];
    });
  }

  async listConflicts(): Promise<SyncConflict[]> {
    return this.database.getAllFromIndex("conflicts", "byAccount", this.accountKey);
  }

  async claimDueMutations(
    leaseOwner: string,
    limit: number,
    leaseDurationMs: number,
  ): Promise<OutboxMutation[]> {
    const now = this.now();
    const transaction = this.database.transaction("outbox", "readwrite");
    const outbox = transaction.objectStore("outbox");
    const all = await outbox.getAll();

    for (const operation of all) {
      if (
        operation.accountKey === this.accountKey &&
        operation.status === "inflight" &&
        operation.leaseExpiresAt !== null &&
        operation.leaseExpiresAt <= now
      ) {
        operation.status = "queued";
        operation.leaseOwner = null;
        operation.leaseExpiresAt = null;
        await outbox.put(operation);
      }
    }

    const due = all
      .filter(
        (operation) =>
          operation.accountKey === this.accountKey &&
          operation.status === "queued" &&
          operation.dependsOnMutationId === null &&
          operation.nextAttemptAt <= now,
      )
      .sort((left, right) => left.localSeq - right.localSeq)
      .slice(0, Math.max(0, limit));

    for (const operation of due) {
      operation.status = "inflight";
      operation.attempts += 1;
      operation.leaseOwner = leaseOwner;
      operation.leaseExpiresAt = now + leaseDurationMs;
      operation.lastError = null;
      await outbox.put(operation);
    }
    await transaction.done;
    return due.map((operation) => structuredClone(operation));
  }

  async retryMutations(
    mutationIds: readonly string[],
    leaseOwner: string,
    delayMs: number,
    error: string,
  ): Promise<void> {
    const transaction = this.database.transaction("outbox", "readwrite");
    const outbox = transaction.objectStore("outbox");
    for (const mutationId of mutationIds) {
      const operation = await outbox.get(outboxKey(this.accountKey, mutationId));
      if (operation?.status !== "inflight" || operation.leaseOwner !== leaseOwner) {
        continue;
      }
      operation.status = "queued";
      operation.nextAttemptAt = this.now() + Math.max(0, delayMs);
      operation.leaseOwner = null;
      operation.leaseExpiresAt = null;
      operation.lastError = error;
      await outbox.put(operation);
    }
    await transaction.done;
  }

  async releaseMutations(mutationIds: readonly string[], leaseOwner: string): Promise<void> {
    const transaction = this.database.transaction("outbox", "readwrite");
    const outbox = transaction.objectStore("outbox");
    for (const mutationId of mutationIds) {
      const operation = await outbox.get(outboxKey(this.accountKey, mutationId));
      if (operation?.status !== "inflight" || operation.leaseOwner !== leaseOwner) {
        continue;
      }
      operation.status = "queued";
      operation.attempts = Math.max(0, operation.attempts - 1);
      operation.nextAttemptAt = this.now();
      operation.leaseOwner = null;
      operation.leaseExpiresAt = null;
      await outbox.put(operation);
    }
    await transaction.done;
  }

  async failMutations(
    mutationIds: readonly string[],
    leaseOwner: string,
    error: string,
  ): Promise<void> {
    const transaction = this.database.transaction(["records", "outbox"], "readwrite");
    const outbox = transaction.objectStore("outbox");
    const records = transaction.objectStore("records");
    for (const mutationId of mutationIds) {
      const operation = await outbox.get(outboxKey(this.accountKey, mutationId));
      if (operation?.status !== "inflight" || operation.leaseOwner !== leaseOwner) {
        continue;
      }
      operation.status = "failed";
      operation.leaseOwner = null;
      operation.leaseExpiresAt = null;
      operation.lastError = error;
      await outbox.put(operation);

      const record = await records.get(recordKey(this.accountKey, operation.key.id));
      if (record !== undefined) {
        record.state = "failed";
        record.updatedAt = this.now();
        await records.put(record);
      }
    }
    await transaction.done;
  }

  async resetPullCursor(): Promise<void> {
    const transaction = this.database.transaction("accountState", "readwrite");
    const states = transaction.objectStore("accountState");
    const current = await states.get(this.accountKey);
    await states.put({
      accountKey: this.accountKey,
      pullCursor: null,
      lastSuccessfulSyncAt: current?.lastSuccessfulSyncAt ?? null,
      lastError: "server rejected the saved sync cursor; replaying from the beginning",
    });
    await transaction.done;
  }

  async applyChanges(response: ChangesResponse): Promise<number> {
    for (const change of response.changes) {
      assertWireRecord(change);
    }
    if (typeof response.nextCursor !== "string" || response.nextCursor.length === 0) {
      throw new TypeError("changes response must include an opaque nextCursor");
    }

    const transaction = this.database.transaction(
      ["records", "conflicts", "accountState"],
      "readwrite",
    );
    const records = transaction.objectStore("records");
    const conflicts = transaction.objectStore("conflicts");
    let changed = 0;
    for (const incoming of response.changes) {
      const key = recordKey(this.accountKey, incoming.key.id);
      const existing = await records.get(key);
      if (
        existing?.confirmed !== null &&
        existing?.confirmed !== undefined &&
        compareDecimalVersions(incoming.version, existing.confirmed.version) <= 0
      ) {
        continue;
      }

      const confirmed = toConfirmed(incoming);
      const conflict = await conflicts.get(key);
      if (conflict !== undefined) {
        conflict.server = confirmed;
        await conflicts.put(conflict);
      }
      await records.put({
        accountKey: this.accountKey,
        kind: "draft_note",
        id: incoming.key.id,
        confirmed,
        optimistic: existing?.optimistic ?? null,
        state:
          existing?.state === "conflict"
            ? "conflict"
            : existing?.state === "failed"
              ? "failed"
              : existing?.optimistic === null || existing?.optimistic === undefined
                ? "synced"
                : "pending",
        updatedAt: this.now(),
      });
      changed += 1;
    }

    await transaction.objectStore("accountState").put({
      accountKey: this.accountKey,
      pullCursor: response.nextCursor,
      lastSuccessfulSyncAt: this.now(),
      lastError: null,
    });
    await transaction.done;
    return changed;
  }

  async applyMutationResults(
    operations: readonly OutboxMutation[],
    results: readonly MutationResult[],
    leaseOwner: string,
  ): Promise<PushApplySummary> {
    const resultById = new Map(results.map((result) => [result.mutationId, result]));
    const summary: PushApplySummary = {
      appliedMutationIds: [],
      conflictedMutationIds: [],
      failedMutationIds: [],
    };
    const transaction = this.database.transaction(
      ["meta", "accountState", "records", "outbox", "conflicts"],
      "readwrite",
    );
    const outbox = transaction.objectStore("outbox");
    const records = transaction.objectStore("records");
    const conflicts = transaction.objectStore("conflicts");

    for (const claimed of operations) {
      const operation = await outbox.get(outboxKey(this.accountKey, claimed.mutationId));
      if (operation?.status !== "inflight" || operation.leaseOwner !== leaseOwner) {
        continue;
      }
      const result = resultById.get(operation.mutationId);
      if (result === undefined) {
        continue;
      }

      const key = recordKey(this.accountKey, operation.key.id);
      const localRecord = await records.get(key);
      if (result.status === "applied") {
        if (result.record === undefined) {
          throw new TypeError("applied mutation result must include record");
        }
        assertWireRecord(result.record);
        if (result.record.key.id !== operation.key.id) {
          throw new TypeError("mutation result record key does not match request");
        }

        await outbox.delete(outboxKey(this.accountKey, operation.mutationId));
        const entityOperations = await outbox
          .index("byAccountEntityLocalSeq")
          .getAll(entityOutboxRange(this.accountKey, operation.key.id));
        const successors = entityOperations.filter(
          (candidate) => candidate.dependsOnMutationId === operation.mutationId,
        );
        for (const successor of successors) {
          await outbox.delete(outboxKey(this.accountKey, successor.mutationId));
          const replacement: OutboxMutation = {
            ...successor,
            mutationId: randomId(),
            baseVersion: result.record.version,
            dependsOnMutationId: null,
            status: "queued",
            attempts: 0,
            nextAttemptAt: this.now(),
            leaseOwner: null,
            leaseExpiresAt: null,
            lastError: null,
          };
          await outbox.put(replacement);
        }

        const hasNewerOptimistic =
          localRecord?.optimistic !== null &&
          localRecord?.optimistic !== undefined &&
          localRecord.optimistic.localSeq > operation.localSeq;
        await records.put({
          accountKey: this.accountKey,
          kind: "draft_note",
          id: operation.key.id,
          confirmed: toConfirmed(result.record),
          optimistic: hasNewerOptimistic ? localRecord.optimistic : null,
          state: hasNewerOptimistic ? "pending" : "synced",
          updatedAt: this.now(),
        });
        await conflicts.delete(key);
        summary.appliedMutationIds.push(operation.mutationId);
        continue;
      }

      if (result.status === "conflict" || result.status === "gone") {
        const serverRecord = result.current ?? result.record;
        if (serverRecord !== undefined) {
          assertWireRecord(serverRecord);
        }
        const local: OptimisticShadow = localRecord?.optimistic ?? {
          localSeq: operation.localSeq,
          action: operation.action,
          ...(operation.value === undefined ? {} : { value: operation.value }),
        };
        const server = serverRecord === undefined ? localRecord?.confirmed ?? null : toConfirmed(serverRecord);
        const conflict: SyncConflict = {
          accountKey: this.accountKey,
          kind: "draft_note",
          id: operation.key.id,
          mutationId: operation.mutationId,
          reason: result.status,
          baseVersion: operation.baseVersion,
          local,
          server,
          detectedAt: this.now(),
        };
        await conflicts.put(conflict);
        operation.status = "blocked";
        operation.leaseOwner = null;
        operation.leaseExpiresAt = null;
        operation.lastError = result.error ?? result.message ?? result.status;
        await outbox.put(operation);

        const entityOperations = await outbox
          .index("byAccountEntityLocalSeq")
          .getAll(entityOutboxRange(this.accountKey, operation.key.id));
        for (const dependent of entityOperations) {
          if (dependent.dependsOnMutationId === operation.mutationId) {
            dependent.status = "blocked";
            dependent.lastError = `blocked by ${result.status}`;
            await outbox.put(dependent);
          }
        }
        await records.put({
          accountKey: this.accountKey,
          kind: "draft_note",
          id: operation.key.id,
          confirmed: server,
          optimistic: local,
          state: "conflict",
          updatedAt: this.now(),
        });
        summary.conflictedMutationIds.push(operation.mutationId);
        continue;
      }

      operation.status = "failed";
      operation.leaseOwner = null;
      operation.leaseExpiresAt = null;
      operation.lastError = result.error ?? result.message ?? result.status;
      await outbox.put(operation);
      if (localRecord !== undefined) {
        localRecord.state = "failed";
        localRecord.updatedAt = this.now();
        await records.put(localRecord);
      }
      summary.failedMutationIds.push(operation.mutationId);
    }
    await transaction.done;
    return summary;
  }

  async acceptServerVersion(id: string): Promise<void> {
    assertRecordId(id);
    const transaction = this.database.transaction(["records", "outbox", "conflicts"], "readwrite");
    const records = transaction.objectStore("records");
    const record = await records.get(recordKey(this.accountKey, id));
    const operations = await transaction
      .objectStore("outbox")
      .index("byAccountEntityLocalSeq")
      .getAll(entityOutboxRange(this.accountKey, id));
    for (const operation of operations) {
      await transaction.objectStore("outbox").delete(outboxKey(this.accountKey, operation.mutationId));
    }
    await transaction.objectStore("conflicts").delete(recordKey(this.accountKey, id));
    if (record !== undefined) {
      record.optimistic = null;
      record.state = "synced";
      record.updatedAt = this.now();
      await records.put(record);
    }
    await transaction.done;
  }

  async discardLocalVersion(id: string): Promise<void> {
    assertRecordId(id);
    const transaction = this.database.transaction(["records", "outbox", "conflicts"], "readwrite");
    const records = transaction.objectStore("records");
    const record = await records.get(recordKey(this.accountKey, id));
    const operations = await transaction
      .objectStore("outbox")
      .index("byAccountEntityLocalSeq")
      .getAll(entityOutboxRange(this.accountKey, id));
    for (const operation of operations) {
      await transaction.objectStore("outbox").delete(outboxKey(this.accountKey, operation.mutationId));
    }
    await transaction.objectStore("conflicts").delete(recordKey(this.accountKey, id));
    if (record !== undefined) {
      if (record.confirmed === null || record.confirmed.deleted) {
        await records.delete(recordKey(this.accountKey, id));
      } else {
        record.optimistic = null;
        record.state = "synced";
        record.updatedAt = this.now();
        await records.put(record);
      }
    }
    await transaction.done;
  }

  toMutationOperations(operations: readonly OutboxMutation[]): MutationOperation[] {
    return operations.map(operationFromOutbox);
  }
}
