import { deleteDB } from "idb";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CanonicalSyncClient } from "../src/sync/engine";
import type { MutationRequest, MutationResponse, WireRecord } from "../src/sync/types";

const ACCOUNT_KEY = "project:user-a";
const NOTE_A = "65e2e9c8-c1b7-46fb-95f4-befcaf47e98d";
const NOTE_B = "8f3502de-754b-4c0e-9e62-f136bb27a457";

class TestBroadcastChannel {
  private static readonly channels = new Map<string, Set<TestBroadcastChannel>>();
  readonly name: string;
  onmessage: ((event: MessageEvent) => void) | null = null;

  constructor(name: string) {
    this.name = name;
    const peers = TestBroadcastChannel.channels.get(name) ?? new Set<TestBroadcastChannel>();
    peers.add(this);
    TestBroadcastChannel.channels.set(name, peers);
  }

  postMessage(message: unknown): void {
    for (const peer of TestBroadcastChannel.channels.get(this.name) ?? []) {
      if (peer !== this) {
        queueMicrotask(() => peer.onmessage?.({ data: structuredClone(message) } as MessageEvent));
      }
    }
  }

  close(): void {
    const peers = TestBroadcastChannel.channels.get(this.name);
    peers?.delete(this);
    if (peers?.size === 0) {
      TestBroadcastChannel.channels.delete(this.name);
    }
  }

  static reset(): void {
    TestBroadcastChannel.channels.clear();
  }
}

const clients: CanonicalSyncClient[] = [];
const databaseNames: string[] = [];

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

async function openClient(
  fetchImplementation: typeof fetch,
  databaseName = `canonical-sync-engine-${crypto.randomUUID()}`,
): Promise<CanonicalSyncClient> {
  const client = await CanonicalSyncClient.bootstrap({
    accountKey: ACCOUNT_KEY,
    databaseName,
    fetch: fetchImplementation,
    visiblePollMs: 60_000,
    random: () => 0,
  });
  clients.push(client);
  databaseNames.push(databaseName);
  return client;
}

async function waitUntil(predicate: () => boolean): Promise<void> {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    if (predicate()) {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
  throw new Error("condition was not reached");
}

async function waitUntilAsync(predicate: () => Promise<boolean>): Promise<void> {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    if (await predicate()) {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
  throw new Error("condition was not reached");
}

beforeEach(() => {
  vi.stubGlobal("BroadcastChannel", TestBroadcastChannel);
});

afterEach(async () => {
  for (const client of clients.splice(0)) {
    await client.close();
  }
  TestBroadcastChannel.reset();
  vi.unstubAllGlobals();
  for (const name of new Set(databaseNames.splice(0))) {
    await deleteDB(name);
  }
});

describe("CanonicalSyncClient coordination", () => {
  it("does not turn pull completion broadcasts into cross-tab pull loops", async () => {
    let requests = 0;
    const fetchImplementation = vi.fn<typeof fetch>(async () => {
      requests += 1;
      return jsonResponse({ changes: [], nextCursor: `cursor-${requests}`, caughtUp: true });
    });
    const databaseName = `canonical-sync-engine-${crypto.randomUUID()}`;
    const first = await openClient(fetchImplementation, databaseName);
    const second = await openClient(fetchImplementation, databaseName);

    // Await each initial pass so runtimes with a real Web Locks API (Node >=
    // 23) release the first client's exclusive lock before the second client
    // requests it with ifAvailable. Polling the request count is insufficient:
    // the fetch may have completed while IndexedDB work still holds the lock.
    await first.start();
    expect(requests).toBe(1);
    await second.start();
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(requests).toBe(2);
  });

  it("drains a local-edit wake that arrives during an active pull", async () => {
    const requestMethods: string[] = [];
    let getRequests = 0;
    let resolveFirstPull: ((response: Response) => void) | undefined;
    const firstPull = new Promise<Response>((resolve) => {
      resolveFirstPull = resolve;
    });
    const fetchImplementation = vi.fn<typeof fetch>(async (_input, init) => {
      const method = init?.method ?? "GET";
      requestMethods.push(method);
      if (method === "POST") {
        const request = JSON.parse(String(init?.body)) as MutationRequest;
        const response: MutationResponse = {
          results: request.operations.map((operation) => ({
            mutationId: operation.mutationId,
            status: "applied",
            record: {
              key: operation.key,
              version: "1",
              schemaVersion: 1,
              deleted: operation.action === "delete",
              ...(operation.value === undefined ? {} : { value: operation.value }),
            },
          })),
        };
        return jsonResponse(response);
      }
      getRequests += 1;
      if (getRequests === 1) {
        return firstPull;
      }
      return jsonResponse({ changes: [], nextCursor: "after-rerun", caughtUp: true });
    });
    const client = await openClient(fetchImplementation);

    client.start();
    await waitUntil(() => getRequests === 1);
    await client.putDraftNote(NOTE_A, { title: "queued mid-pull", body: "must push promptly" });
    resolveFirstPull?.(
      jsonResponse({ changes: [], nextCursor: "first-pass", caughtUp: true }),
    );
    await waitUntilAsync(
      async () => getRequests === 2 && (await client.store.listOutbox()).length === 0,
    );

    expect(requestMethods).toEqual(["GET", "POST", "GET"]);
    expect(await client.store.listOutbox()).toEqual([]);
    expect(await client.store.getRecord(NOTE_A)).toMatchObject({
      state: "synced",
      confirmed: {
        value: { title: "queued mid-pull", body: "must push promptly" },
      },
    });
  });

  it("keeps the active promise pending for a wake at the drain-settlement edge", async () => {
    vi.stubGlobal("navigator", { onLine: true });
    let getRequests = 0;
    let resolveSecondPull: ((response: Response) => void) | undefined;
    const secondPull = new Promise<Response>((resolve) => {
      resolveSecondPull = resolve;
    });
    const fetchImplementation = vi.fn<typeof fetch>(async () => {
      getRequests += 1;
      if (getRequests === 2) {
        return secondPull;
      }
      return jsonResponse({ changes: [], nextCursor: `cursor-${getRequests}`, caughtUp: true });
    });
    const client = await openClient(fetchImplementation);
    let wakeScheduled = false;
    client.addEventListener("change", () => {
      if (wakeScheduled) {
        return;
      }
      wakeScheduled = true;
      // Run after pullOnce, syncPass, withLeaderLock, and drainSyncRequests
      // have each resumed, but before the drain's finally callback settles.
      queueMicrotask(() =>
        queueMicrotask(() =>
          queueMicrotask(() => queueMicrotask(() => void client.syncNow())),
        ),
      );
    });

    let settled = false;
    const sync = client.syncNow().then(() => {
      settled = true;
    });
    await waitUntil(() => getRequests === 2);
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(settled).toBe(false);
    resolveSecondPull?.(
      jsonResponse({ changes: [], nextCursor: "after-edge-wake", caughtUp: true }),
    );
    await sync;
    expect(settled).toBe(true);
  });

  it("cannot restore account data from a pull that completes during logout", async () => {
    let resolvePull: ((response: Response) => void) | undefined;
    let pullStarted = false;
    const fetchImplementation = vi.fn<typeof fetch>(
      () =>
        new Promise<Response>((resolve) => {
          pullStarted = true;
          resolvePull = resolve;
        }),
    );
    const client = await openClient(fetchImplementation);
    client.start();
    await waitUntil(() => pullStarted);

    const clearing = client.clearLocalData();
    resolvePull?.(
      jsonResponse({
        changes: [
          {
            key: { kind: "draft_note", id: NOTE_A },
            version: "1",
            schemaVersion: 1,
            deleted: false,
            value: { title: "private", body: "must stay purged" },
          } satisfies WireRecord,
        ],
        nextCursor: "late-cursor",
        caughtUp: true,
      }),
    );
    await clearing;

    expect(await client.store.getRecord(NOTE_A)).toBeUndefined();
    expect(await client.store.listOutbox()).toEqual([]);
  });

  it("drops an invalid opaque cursor and performs a baseline pull", async () => {
    let requests = 0;
    const fetchImplementation = vi.fn<typeof fetch>(async () => {
      requests += 1;
      if (requests === 1) {
        return jsonResponse(
          { error: { code: "invalid_sync_cursor", message: "invalid sync cursor" } },
          400,
        );
      }
      return jsonResponse({ changes: [], nextCursor: "fresh-cursor", caughtUp: true });
    });
    const client = await openClient(fetchImplementation);
    await client.store.applyChanges({ changes: [], nextCursor: "stale-cursor", caughtUp: true });

    await client.syncNow();

    expect(requests).toBe(2);
    expect((await client.store.getAccountState()).pullCursor).toBe("fresh-cursor");
  });

  it("marks permanent HTTP mutation failures instead of retrying forever", async () => {
    let posts = 0;
    const fetchImplementation = vi.fn<typeof fetch>(async (_input, init) => {
      if (init?.method === "POST") {
        posts += 1;
        return jsonResponse({ error: { code: "bad_request", message: "invalid batch" } }, 422);
      }
      return jsonResponse({ changes: [], nextCursor: "cursor", caughtUp: true });
    });
    const client = await openClient(fetchImplementation);
    await client.putDraftNote(NOTE_A, { title: "local", body: "body" });

    await client.syncNow();
    await client.syncNow();

    expect(posts).toBe(1);
    expect(await client.store.listOutbox()).toEqual([
      expect.objectContaining({ status: "failed", lastError: "invalid batch" }),
    ]);
    expect(await client.store.getRecord(NOTE_A)).toMatchObject({ state: "failed" });
  });

  it("splits valid large notes by encoded request bytes and releases the excess lease", async () => {
    const postSizes: number[] = [];
    const fetchImplementation = vi.fn<typeof fetch>(async (_input, init) => {
      if (init?.method !== "POST") {
        return jsonResponse({ changes: [], nextCursor: "cursor", caughtUp: true });
      }
      const encoded = String(init.body);
      postSizes.push(new TextEncoder().encode(encoded).byteLength);
      const request = JSON.parse(encoded) as MutationRequest;
      const response: MutationResponse = {
        results: request.operations.map((operation, index) => ({
          mutationId: operation.mutationId,
          status: "applied",
          record: {
            key: operation.key,
            version: String(index + 1),
            schemaVersion: 1,
            deleted: operation.action === "delete",
            ...(operation.value === undefined ? {} : { value: operation.value }),
          },
        })),
      };
      return jsonResponse(response);
    });
    const client = await openClient(fetchImplementation);
    const largeBody = "\u0001".repeat(100_000);
    await client.putDraftNote(NOTE_A, { title: "first", body: largeBody });
    await client.putDraftNote(NOTE_B, { title: "second", body: largeBody });

    await client.syncNow();

    expect(postSizes).toHaveLength(2);
    expect(postSizes.every((size) => size <= 1_000_000)).toBe(true);
    expect(await client.store.listOutbox()).toEqual([]);
  });
});
