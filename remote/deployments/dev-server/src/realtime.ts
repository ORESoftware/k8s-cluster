// Supabase Realtime publisher for the dev-server.
//
// The remote dev server publishes task events to a per-user Supabase
// Broadcast channel. Channels are pooled so a task stream does not pay a
// subscribe/unsubscribe cost for every event, and publishes are serialized
// per user to preserve event order.

import {
  createClient,
  type RealtimeChannel,
  type SupabaseClient,
} from "@supabase/supabase-js";

let cachedClient: SupabaseClient | null = null;
let clientRetries = 0;

const MAX_CLIENT_RETRIES = 5;
const CHANNEL_IDLE_GRACE_MS = 60_000;
const CHANNEL_STALE_MS = 90_000;
const CHANNEL_HEALTH_INTERVAL_MS = 30_000;
const CHANNEL_FAILURE_THRESHOLD = 5;

type ServerBroadcastMessage = {
  type: "custom";
  payload: unknown;
  senderId?: string;
  timestamp: number;
};

type PoolEntry = {
  channel: RealtimeChannel;
  ready: boolean;
  readyPromise: Promise<void>;
  refCount: number;
  lastSendAt: number;
  consecutiveFailures: number;
  teardownTimer: ReturnType<typeof setTimeout> | null;
  queue: Promise<void>;
};

const channelPool = new Map<string, PoolEntry>();
let healthTimer: ReturnType<typeof setInterval> | null = null;

function getClient(): SupabaseClient | null {
  if (cachedClient) {
    return cachedClient;
  }
  const url = process.env.SUPABASE_URL ?? process.env.NEXT_PUBLIC_SUPABASE_URL;
  const key = process.env.SUPABASE_SERVICE_ROLE_KEY;
  if (!url || !key) {
    return null;
  }
  try {
    cachedClient = createClient(url, key, {
      auth: { persistSession: false, autoRefreshToken: false },
      realtime: { params: { eventsPerSecond: 50 } },
    });
    clientRetries = 0;
    return cachedClient;
  } catch (err) {
    clientRetries += 1;
    process.stderr.write(
      `[remote-dev realtime] client creation failed (attempt ${clientRetries}/${MAX_CLIENT_RETRIES}): ${
        err instanceof Error ? err.message : String(err)
      }\n`,
    );
    if (clientRetries >= MAX_CLIENT_RETRIES) {
      process.stderr.write(
        "[remote-dev realtime] max client retries reached; disabling Realtime\n",
      );
    }
    return null;
  }
}

export function isRealtimeEnabled(): boolean {
  return getClient() !== null;
}

export function getRemoteDevUserChannelName(userId: string): string {
  return `remote-dev:user:${userId}`;
}

function startHealthCheck(): void {
  if (healthTimer) {
    return;
  }
  healthTimer = setInterval(() => {
    const now = Date.now();
    for (const [userId, entry] of channelPool) {
      const stale =
        entry.ready &&
        entry.lastSendAt > 0 &&
        now - entry.lastSendAt > CHANNEL_STALE_MS;
      const unhealthy =
        entry.consecutiveFailures >= CHANNEL_FAILURE_THRESHOLD;
      if (stale || unhealthy) {
        process.stderr.write(
          `[remote-dev realtime] recycling channel for user ${userId.slice(
            0,
            8,
          )} (stale=${stale}, failures=${entry.consecutiveFailures})\n`,
        );
        void removeChannel(userId, entry);
      }
    }
  }, CHANNEL_HEALTH_INTERVAL_MS);
}

async function removeChannel(userId: string, entry: PoolEntry): Promise<void> {
  if (entry.teardownTimer) {
    clearTimeout(entry.teardownTimer);
    entry.teardownTimer = null;
  }
  const client = getClient();
  if (client) {
    await client.removeChannel(entry.channel).catch(() => undefined);
  }
  if (channelPool.get(userId) === entry) {
    channelPool.delete(userId);
  }
}

function createEntry(userId: string): PoolEntry | null {
  const client = getClient();
  if (!client) {
    return null;
  }

  const channelName = getRemoteDevUserChannelName(userId);
  const channel = client.channel(channelName);
  const entry: PoolEntry = {
    channel,
    ready: false,
    readyPromise: Promise.resolve(),
    refCount: 0,
    lastSendAt: 0,
    consecutiveFailures: 0,
    teardownTimer: null,
    queue: Promise.resolve(),
  };

  entry.readyPromise = new Promise<void>((resolve, reject) => {
    let settled = false;
    const subscribeTimeout = setTimeout(() => {
      if (settled) {
        return;
      }
      settled = true;
      void removeChannel(userId, entry);
      reject(new Error(`channel subscribe timeout: ${channelName}`));
    }, 15_000);

    channel.subscribe((status) => {
      if (settled) {
        return;
      }
      if (status === "SUBSCRIBED") {
        settled = true;
        clearTimeout(subscribeTimeout);
        entry.ready = true;
        resolve();
      } else if (status === "CLOSED" || status === "CHANNEL_ERROR") {
        settled = true;
        clearTimeout(subscribeTimeout);
        entry.ready = false;
        void removeChannel(userId, entry);
        reject(new Error(`channel subscription failed: ${status}`));
      }
    });
  });

  channelPool.set(userId, entry);
  startHealthCheck();
  return entry;
}

function getOrCreateEntry(userId: string): PoolEntry | null {
  const existing = channelPool.get(userId);
  if (existing) {
    if (existing.teardownTimer) {
      clearTimeout(existing.teardownTimer);
      existing.teardownTimer = null;
    }
    return existing;
  }
  return createEntry(userId);
}

async function sendOnChannel(
  entry: PoolEntry,
  payload: unknown,
  senderId?: string,
): Promise<void> {
  await entry.readyPromise;

  const message: ServerBroadcastMessage = {
    type: "custom",
    payload,
    senderId,
    timestamp: Date.now(),
  };

  const result = await entry.channel.send({
    type: "broadcast",
    event: "message",
    payload: message,
  });

  if (result !== "ok") {
    throw new Error(`broadcast send returned ${String(result)}`);
  }
  entry.lastSendAt = Date.now();
  entry.consecutiveFailures = 0;
}

export function acquireUserChannel(userId: string): void {
  const entry = getOrCreateEntry(userId);
  if (!entry) {
    return;
  }
  entry.refCount += 1;
}

export function releaseUserChannel(userId: string): void {
  const entry = channelPool.get(userId);
  if (!entry) {
    return;
  }
  entry.refCount = Math.max(0, entry.refCount - 1);
  if (entry.refCount > 0 || entry.teardownTimer) {
    return;
  }
  entry.teardownTimer = setTimeout(() => {
    const current = channelPool.get(userId);
    if (current === entry && current.refCount === 0) {
      void removeChannel(userId, entry);
    }
  }, CHANNEL_IDLE_GRACE_MS);
}

export async function publishUserEvent(
  userId: string,
  payload: unknown,
  senderId?: string,
): Promise<void> {
  const entry = getOrCreateEntry(userId);
  if (!entry) {
    return;
  }

  const previous = entry.queue.catch(() => undefined);
  const publish = previous
    .then(() => sendOnChannel(entry, payload, senderId))
    .catch(async (err: unknown) => {
      entry.consecutiveFailures += 1;
      process.stderr.write(
        `[remote-dev realtime] publish failed: ${
          err instanceof Error ? err.message : String(err)
        }\n`,
      );
      if (entry.consecutiveFailures >= CHANNEL_FAILURE_THRESHOLD) {
        await removeChannel(userId, entry);
      }
    });
  entry.queue = publish;
  await publish;
}

export function destroyChannelPool(): void {
  if (healthTimer) {
    clearInterval(healthTimer);
    healthTimer = null;
  }
  const entries = Array.from(channelPool.entries());
  for (const [userId, entry] of entries) {
    void removeChannel(userId, entry);
  }
  channelPool.clear();
}

export function destroyAllChannels(): void {
  destroyChannelPool();
}
