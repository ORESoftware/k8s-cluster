import { deleteDB } from "idb";
import { afterEach, describe, expect, it } from "vitest";
import { SyncStore } from "../src/sync/store";
import type { DraftNoteValue, WireRecord } from "../src/sync/types";

const openStores: Array<{ name: string; store: SyncStore }> = [];
const NOTE_ID = "65e2e9c8-c1b7-46fb-95f4-befcaf47e98d";

async function openStore(accountKey = "project:user-a"): Promise<SyncStore> {
  const name = `canonical-sync-test-${crypto.randomUUID()}`;
  const store = await SyncStore.open({ accountKey, databaseName: name, now: () => 1_000 });
  openStores.push({ name, store });
  return store;
}

function note(title: string, body = "body"): DraftNoteValue {
  return { title, body };
}

function wireRecord(id: string, version: string, value: DraftNoteValue): WireRecord {
  return {
    key: { kind: "draft_note", id },
    version,
    schemaVersion: 1,
    deleted: false,
    value,
  };
}

function deletedWireRecord(id: string, version: string): WireRecord {
  return {
    key: { kind: "draft_note", id },
    version,
    schemaVersion: 1,
    deleted: true,
  };
}

afterEach(async () => {
  const opened = openStores.splice(0);
  for (const { store } of opened) {
    store.close();
  }
  for (const name of new Set(opened.map(({ name }) => name))) {
    await deleteDB(name);
  }
});

describe("SyncStore optimistic mutations", () => {
  it("atomically writes an optimistic record and its outbox operation", async () => {
    const store = await openStore();

    const mutation = await store.putDraftNote(NOTE_ID, note("local"));
    const record = await store.getRecord(NOTE_ID);
    const outbox = await store.listOutbox();

    expect(record).toMatchObject({
      accountKey: "project:user-a",
      kind: "draft_note",
      id: NOTE_ID,
      confirmed: null,
      optimistic: { action: "put", value: note("local") },
      state: "pending",
    });
    expect(outbox).toHaveLength(1);
    expect(outbox[0]).toMatchObject({
      mutationId: mutation.mutationId,
      baseVersion: null,
      action: "put",
      value: note("local"),
      status: "queued",
    });
  });

  it("does not leave a partial record when validation rejects a mutation", async () => {
    const store = await openStore();

    await expect(store.putDraftNote(NOTE_ID, note("x".repeat(201)))).rejects.toThrow(
      "title must be at most 200",
    );

    expect(await store.getRecord(NOTE_ID)).toBeUndefined();
    expect(await store.listOutbox()).toEqual([]);
  });

  it("uses Unicode scalar values for the shared title limit", async () => {
    const store = await openStore();

    await expect(
      store.putDraftNote(NOTE_ID, note("😀".repeat(200))),
    ).resolves.toBeDefined();
    await expect(
      store.putDraftNote("8f3502de-754b-4c0e-9e62-f136bb27a457", note("😀".repeat(201))),
    ).rejects.toThrow("title must be at most 200");
  });

  it("removes account-scoped cached data on logout", async () => {
    const store = await openStore();
    await store.putDraftNote(NOTE_ID, note("private offline data"));

    await store.clearAccount();

    expect(await store.getRecord(NOTE_ID)).toBeUndefined();
    expect(await store.listOutbox()).toEqual([]);
  });

  it("purges the previous account when a different user takes over the browser", async () => {
    const name = `canonical-sync-test-${crypto.randomUUID()}`;
    const first = await SyncStore.open({ accountKey: "project:user-a", databaseName: name });
    openStores.push({ name, store: first });
    await first.putDraftNote(NOTE_ID, note("belongs to user A"));
    first.close();

    const second = await SyncStore.open({ accountKey: "project:user-b", databaseName: name });
    openStores.push({ name, store: second });

    expect(
      await second.database.get("records", ["project:user-a", "draft_note", NOTE_ID]),
    ).toBeUndefined();
  });

  it("never changes the semantic payload associated with an inflight mutation id", async () => {
    const store = await openStore();
    const first = await store.putDraftNote(NOTE_ID, note("first"));
    const [claimed] = await store.claimDueMutations("worker", 10, 30_000);
    expect(claimed?.mutationId).toBe(first.mutationId);

    const second = await store.putDraftNote(NOTE_ID, note("second"));
    const outbox = await store.listOutbox();
    const persistedFirst = outbox.find((operation) => operation.mutationId === first.mutationId);
    const successor = outbox.find((operation) => operation.mutationId === second.mutationId);

    expect(persistedFirst).toMatchObject({
      status: "inflight",
      baseVersion: null,
      value: note("first"),
    });
    expect(successor).toMatchObject({
      status: "queued",
      dependsOnMutationId: first.mutationId,
      value: note("second"),
    });
  });
});

describe("SyncStore pull and conflicts", () => {
  it("updates the confirmed shadow and cursor without clobbering an optimistic edit", async () => {
    const store = await openStore();
    await store.putDraftNote(NOTE_ID, note("local"));

    await store.applyChanges({
      changes: [wireRecord(NOTE_ID, "900719925474099312345", note("remote"))],
      nextCursor: "opaque-cursor-1",
      caughtUp: true,
    });

    const record = await store.getRecord(NOTE_ID);
    expect(record?.confirmed).toMatchObject({
      version: "900719925474099312345",
      value: note("remote"),
    });
    expect(record?.optimistic).toMatchObject({ action: "put", value: note("local") });
    expect(record?.state).toBe("pending");
    expect((await store.getAccountState()).pullCursor).toBe("opaque-cursor-1");
    expect(await store.listEffectiveDraftNotes()).toEqual([
      {
        id: NOTE_ID,
        value: note("local"),
        version: "900719925474099312345",
        pending: true,
        conflicted: false,
        failed: false,
      },
    ]);
  });

  it("stores the local and server shadows when a push conflicts", async () => {
    const store = await openStore();
    const mutation = await store.putDraftNote(NOTE_ID, note("local"));
    const claimed = await store.claimDueMutations("worker", 10, 30_000);

    const summary = await store.applyMutationResults(
      claimed,
      [
        {
          mutationId: mutation.mutationId,
          status: "conflict",
          current: wireRecord(NOTE_ID, "7", note("server")),
        },
      ],
      "worker",
    );

    expect(summary.conflictedMutationIds).toEqual([mutation.mutationId]);
    expect(await store.getConflict(NOTE_ID)).toMatchObject({
      reason: "conflict",
      baseVersion: null,
      local: { action: "put", value: note("local") },
      server: { version: "7", value: note("server") },
    });
    expect(await store.getRecord(NOTE_ID)).toMatchObject({
      state: "conflict",
      confirmed: { version: "7", value: note("server") },
      optimistic: { action: "put", value: note("local") },
    });
    expect(await store.listOutbox()).toEqual([
      expect.objectContaining({ mutationId: mutation.mutationId, status: "blocked" }),
    ]);

    await store.acceptServerVersion(NOTE_ID);

    expect(await store.getConflict(NOTE_ID)).toBeUndefined();
    expect(await store.listOutbox()).toEqual([]);
    expect(await store.getRecord(NOTE_ID)).toMatchObject({
      state: "synced",
      confirmed: { version: "7", value: note("server") },
      optimistic: null,
    });
  });

  it("ignores stale pulled versions using decimal string ordering", async () => {
    const store = await openStore();
    await store.applyChanges({
      changes: [wireRecord(NOTE_ID, "10", note("new"))],
      nextCursor: "cursor-1",
      caughtUp: false,
    });
    await store.applyChanges({
      changes: [wireRecord(NOTE_ID, "9", note("old"))],
      nextCursor: "cursor-2",
      caughtUp: true,
    });

    expect(await store.getRecord(NOTE_ID)).toMatchObject({
      confirmed: { version: "10", value: note("new") },
    });
    expect((await store.getAccountState()).pullCursor).toBe("cursor-2");
  });

  it("does not resurrect a persisted tombstone after the browser database reopens", async () => {
    const name = `canonical-sync-test-${crypto.randomUUID()}`;
    const first = await SyncStore.open({ accountKey: "project:user-a", databaseName: name });
    openStores.push({ name, store: first });
    await first.applyChanges({
      changes: [deletedWireRecord(NOTE_ID, "10")],
      nextCursor: "cursor-tombstone",
      caughtUp: true,
    });
    first.close();

    const restarted = await SyncStore.open({ accountKey: "project:user-a", databaseName: name });
    openStores.push({ name, store: restarted });
    await restarted.applyChanges({
      changes: [wireRecord(NOTE_ID, "9", note("stale payload"))],
      nextCursor: "cursor-replayed",
      caughtUp: true,
    });

    expect(await restarted.getRecord(NOTE_ID)).toMatchObject({
      confirmed: { version: "10", deleted: true },
      optimistic: null,
      state: "synced",
    });
    expect(await restarted.listEffectiveDraftNotes()).toEqual([]);
  });
});
