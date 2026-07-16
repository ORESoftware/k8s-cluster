import { retryDelayMs } from "./backoff";
import { randomId } from "./id";
import { SyncStore, type OpenSyncStoreOptions } from "./store";
import {
  encodedMutationRequestBytes,
  SyncHttpError,
  SyncTransport,
  type SyncTransportOptions,
} from "./transport";
import type { DraftNoteValue, OutboxMutation, SyncChangeNotification } from "./types";
import { SyncInvalidationSocket } from "./websocket";

const DEFAULT_PULL_LIMIT = 200;
const DEFAULT_PUSH_LIMIT = 50;
const DEFAULT_LEASE_MS = 30_000;
const DEFAULT_VISIBLE_POLL_MS = 30_000;
const MAX_PUSH_BODY_BYTES = 1_000_000;

export interface CanonicalSyncClientOptions extends OpenSyncStoreOptions, SyncTransportOptions {
  pullLimit?: number;
  pushLimit?: number;
  leaseDurationMs?: number;
  visiblePollMs?: number;
  websocketPath?: string;
  onAuthRequired?: () => void;
  onAccessRevoked?: () => void;
  random?: () => number;
}

export class CanonicalSyncClient extends EventTarget {
  readonly store: SyncStore;
  private readonly transport: SyncTransport;
  private readonly options: CanonicalSyncClientOptions;
  private readonly leaseOwner = randomId();
  private readonly broadcastChannel: BroadcastChannel | null;
  private readonly socket: SyncInvalidationSocket;
  private readonly htmxOwnsSocket: boolean;
  private runningSync: Promise<void> | null = null;
  private pendingSync = false;
  private activeAbortController: AbortController | null = null;
  private syncGeneration = 0;
  private pollTimer: ReturnType<typeof setInterval> | null = null;
  private started = false;

  private constructor(store: SyncStore, options: CanonicalSyncClientOptions) {
    super();
    this.store = store;
    this.options = options;
    this.transport = new SyncTransport(options);
    this.htmxOwnsSocket =
      typeof document !== "undefined" &&
      document.querySelector('[hx-ext~="ws"][ws-connect="/ws"]') !== null;
    this.broadcastChannel =
      typeof BroadcastChannel === "undefined"
        ? null
        : new BroadcastChannel(`canonical-sync:${options.accountKey}`);
    if (this.broadcastChannel !== null) {
      this.broadcastChannel.onmessage = (event: MessageEvent<SyncChangeNotification>) => {
        if (event.data.accountKey === this.store.accountKey) {
          if (event.data.source === "logout") {
            void this.handleRemoteLogout();
            return;
          }
          this.emitChange(event.data.source);
          if (event.data.source === "local" && this.started) {
            void this.syncNow();
          }
        }
      };
    }
    this.socket = new SyncInvalidationSocket({
      onInvalidate: () => void this.syncNow(),
      path: options.websocketPath ?? "/ws",
      ...(options.onAccessRevoked === undefined && options.onAuthRequired === undefined
        ? {}
        : { onAccessRevoked: options.onAccessRevoked ?? options.onAuthRequired }),
      ...(options.random === undefined ? {} : { random: options.random }),
    });
  }

  static async bootstrap(options: CanonicalSyncClientOptions): Promise<CanonicalSyncClient> {
    const store = await SyncStore.open(options);
    return new CanonicalSyncClient(store, options);
  }

  start(): Promise<void> {
    if (this.started) {
      return this.runningSync ?? Promise.resolve();
    }
    this.started = true;
    globalThis.addEventListener?.("online", this.handleWake);
    if (typeof document !== "undefined") {
      document.addEventListener("visibilitychange", this.handleVisibilityChange);
    }
    this.pollTimer = setInterval(
      () => {
        if (typeof document === "undefined" || document.visibilityState === "visible") {
          void this.syncNow();
        }
      },
      this.options.visiblePollMs ?? DEFAULT_VISIBLE_POLL_MS,
    );
    if (!this.htmxOwnsSocket) {
      this.socket.start();
    }
    return this.syncNow();
  }

  stop(): void {
    if (!this.started) {
      return;
    }
    this.started = false;
    globalThis.removeEventListener?.("online", this.handleWake);
    if (typeof document !== "undefined") {
      document.removeEventListener("visibilitychange", this.handleVisibilityChange);
    }
    if (this.pollTimer !== null) {
      clearInterval(this.pollTimer);
      this.pollTimer = null;
    }
    this.socket.stop();
  }

  async close(): Promise<void> {
    this.stop();
    await this.cancelAndWaitForActiveSync();
    this.broadcastChannel?.close();
    this.store.close();
  }

  async clearLocalData(): Promise<void> {
    this.stop();
    await this.cancelAndWaitForActiveSync();
    await this.store.clearAccount();
    this.broadcastChannel?.postMessage({
      accountKey: this.store.accountKey,
      source: "logout",
    } satisfies SyncChangeNotification);
  }

  async putDraftNote(id: string, value: DraftNoteValue): Promise<void> {
    await this.store.putDraftNote(id, value);
    this.announce("local");
    if (this.started) {
      void this.syncNow();
    }
  }

  async deleteDraftNote(id: string): Promise<void> {
    await this.store.deleteDraftNote(id);
    this.announce("local");
    if (this.started) {
      void this.syncNow();
    }
  }

  async acceptServerVersion(id: string): Promise<void> {
    await this.store.acceptServerVersion(id);
    this.announce("conflict");
  }

  async discardLocalVersion(id: string): Promise<void> {
    await this.store.discardLocalVersion(id);
    this.announce("push");
  }

  async syncNow(): Promise<void> {
    if (this.runningSync !== null) {
      this.pendingSync = true;
      return this.runningSync;
    }
    const generation = this.syncGeneration;
    const controller = new AbortController();
    this.activeAbortController = controller;
    const running = this.drainSyncRequests(generation, controller.signal).finally(() => {
      let restart = false;
      if (this.runningSync === running) {
        this.runningSync = null;
        restart = this.pendingSync && this.isCurrentSync(generation, controller.signal);
      }
      if (this.activeAbortController === controller) {
        this.activeAbortController = null;
      }
      // A wake can land after the drain loop's final check but before this
      // promise settles. Preserve that edge-trigger by starting a new drain.
      if (restart) {
        return this.syncNow();
      }
    });
    this.runningSync = running;
    return running;
  }

  private async drainSyncRequests(generation: number, signal: AbortSignal): Promise<void> {
    do {
      this.pendingSync = false;
      await this.withLeaderLock(generation, signal);
    } while (this.pendingSync && this.isCurrentSync(generation, signal));
  }

  private readonly handleWake = (): void => {
    void this.syncNow();
  };

  private readonly handleVisibilityChange = (): void => {
    if (document.visibilityState === "visible") {
      void this.syncNow();
    }
  };

  private async withLeaderLock(generation: number, signal: AbortSignal): Promise<void> {
    const locks = globalThis.navigator?.locks;
    if (locks === undefined) {
      await this.syncPass(generation, signal);
      return;
    }
    await locks.request(
      `canonical-sync:${this.store.accountKey}`,
      { mode: "exclusive", ifAvailable: true },
      async (lock) => {
        if (lock !== null) {
          await this.syncPass(generation, signal);
        }
      },
    );
  }

  private async syncPass(generation: number, signal: AbortSignal): Promise<void> {
    if (
      !this.isCurrentSync(generation, signal) ||
      (typeof navigator !== "undefined" && navigator.onLine === false)
    ) {
      return;
    }
    for (let batch = 0; batch < 100; batch += 1) {
      const pushed = await this.pushOnce(generation, signal);
      if (pushed === 0 || !this.isCurrentSync(generation, signal)) {
        break;
      }
    }
    for (let page = 0; page < 100; page += 1) {
      const caughtUp = await this.pullOnce(generation, signal);
      if (caughtUp || !this.isCurrentSync(generation, signal)) {
        break;
      }
    }
  }

  private async pushOnce(generation: number, signal: AbortSignal): Promise<number> {
    const claimed = await this.store.claimDueMutations(
      this.leaseOwner,
      this.options.pushLimit ?? DEFAULT_PUSH_LIMIT,
      this.options.leaseDurationMs ?? DEFAULT_LEASE_MS,
    );
    if (claimed.length === 0) {
      return 0;
    }
    if (!this.isCurrentSync(generation, signal)) {
      await this.store.releaseMutations(
        claimed.map((operation) => operation.mutationId),
        this.leaseOwner,
      );
      return 0;
    }
    const operations = this.selectPushBatch(claimed);
    const deferred = claimed.slice(operations.length);
    if (deferred.length > 0) {
      await this.store.releaseMutations(
        deferred.map((operation) => operation.mutationId),
        this.leaseOwner,
      );
    }
    try {
      const response = await this.transport.pushMutations(this.store.clientId, operations, signal);
      if (!this.isCurrentSync(generation, signal)) {
        await this.store.releaseMutations(
          operations.map((operation) => operation.mutationId),
          this.leaseOwner,
        );
        return 0;
      }
      await this.store.applyMutationResults(operations, response.results, this.leaseOwner);
      const returnedIds = new Set(response.results.map((result) => result.mutationId));
      const missing = operations.filter((operation) => !returnedIds.has(operation.mutationId));
      if (missing.length > 0) {
        await this.store.retryMutations(
          missing.map((operation) => operation.mutationId),
          this.leaseOwner,
          retryDelayMs(Math.max(...missing.map((operation) => operation.attempts)), this.options.random),
          "server omitted mutation result",
        );
      }
      this.announce(response.results.some((result) => result.status === "conflict" || result.status === "gone") ? "conflict" : "push");
      return operations.length;
    } catch (error) {
      if (!this.isCurrentSync(generation, signal)) {
        await this.store.releaseMutations(
          operations.map((operation) => operation.mutationId),
          this.leaseOwner,
        );
        return 0;
      }
      const message = error instanceof Error ? error.message : "sync push failed";
      if (error instanceof SyncHttpError && !error.retryable && error.status !== 401) {
        await this.store.failMutations(
          operations.map((operation) => operation.mutationId),
          this.leaseOwner,
          message,
        );
        this.announce("push");
        return 0;
      }
      const maxAttempt = Math.max(...operations.map((operation) => operation.attempts));
      const defaultDelay = retryDelayMs(maxAttempt, this.options.random);
      const delay = error instanceof SyncHttpError && error.retryAfterMs !== null ? error.retryAfterMs : defaultDelay;
      await this.store.retryMutations(
        operations.map((operation) => operation.mutationId),
        this.leaseOwner,
        delay,
        message,
      );
      if (error instanceof SyncHttpError && error.status === 401) {
        this.options.onAuthRequired?.();
      }
      return 0;
    }
  }

  private async pullOnce(generation: number, signal: AbortSignal): Promise<boolean> {
    try {
      const state = await this.store.getAccountState();
      const response = await this.transport.getChanges(
        state.pullCursor,
        this.options.pullLimit ?? DEFAULT_PULL_LIMIT,
        signal,
      );
      if (!this.isCurrentSync(generation, signal)) {
        return true;
      }
      await this.store.applyChanges(response);
      this.announce("pull");
      return response.caughtUp;
    } catch (error) {
      if (!this.isCurrentSync(generation, signal)) {
        return true;
      }
      if (error instanceof SyncHttpError && error.code === "invalid_sync_cursor") {
        await this.store.resetPullCursor();
        return false;
      }
      if (error instanceof SyncHttpError && error.status === 401) {
        this.options.onAuthRequired?.();
      }
      return true;
    }
  }

  private selectPushBatch(claimed: OutboxMutation[]): OutboxMutation[] {
    const selected: OutboxMutation[] = [];
    for (const operation of claimed) {
      const candidate = [...selected, operation];
      if (
        selected.length > 0 &&
        encodedMutationRequestBytes(this.store.clientId, candidate) > MAX_PUSH_BODY_BYTES
      ) {
        break;
      }
      selected.push(operation);
    }
    return selected;
  }

  private isCurrentSync(generation: number, signal: AbortSignal): boolean {
    return generation === this.syncGeneration && !signal.aborted;
  }

  private cancelActiveSync(): Promise<void> | null {
    this.pendingSync = false;
    this.syncGeneration += 1;
    this.activeAbortController?.abort();
    return this.runningSync;
  }

  private async cancelAndWaitForActiveSync(): Promise<void> {
    const running = this.cancelActiveSync();
    if (running !== null) {
      try {
        await running;
      } catch {
        // Cancellation is expected while logging out; the account is purged below.
      }
    }
  }

  private async handleRemoteLogout(): Promise<void> {
    this.stop();
    await this.cancelAndWaitForActiveSync();
    await this.store.clearAccount();
    this.emitChange("logout");
  }

  private announce(source: SyncChangeNotification["source"]): void {
    const notification: SyncChangeNotification = { accountKey: this.store.accountKey, source };
    this.broadcastChannel?.postMessage(notification);
    this.emitChange(source);
  }

  private emitChange(source: SyncChangeNotification["source"]): void {
    this.dispatchEvent(
      new CustomEvent<SyncChangeNotification>("change", {
        detail: { accountKey: this.store.accountKey, source },
      }),
    );
  }
}
