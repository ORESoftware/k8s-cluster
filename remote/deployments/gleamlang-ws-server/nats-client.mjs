import net from 'node:net';

const DEFAULT_RECONNECT_MS = 1000;
const MAX_QUEUE_DEPTH = 500;

let singleton = null;

export function getNatsClient(options = {}) {
  const url = options.url ?? process.env.NATS_URL ?? null;
  if (!singleton || singleton.url !== url) {
    singleton?.destroy();
    singleton = new NatsClient({
      url,
      logger: options.logger ?? console,
      reconnectMs: options.reconnectMs ?? DEFAULT_RECONNECT_MS,
    });
  }
  return singleton;
}

class NatsClient {
  constructor({ url, logger, reconnectMs }) {
    this.url = url;
    this.logger = logger;
    this.reconnectMs = reconnectMs;
    this.socket = null;
    this.connecting = false;
    this.connected = false;
    this.reconnectTimer = null;
    this.buffer = Buffer.alloc(0);
    this.queue = [];
    this.subscriptions = new Map();
    this.nextSid = 1;
  }

  subscribe(subject, handler) {
    if (!this.url) {
      this.logger.warn('[nats-client] subscribe disabled: NATS_URL is not configured');
      return () => {};
    }
    assertSubject(subject, 'subscribe subject');
    const sid = this.nextSid++;
    this.subscriptions.set(sid, { subject, handler });
    if (this.connected && this.socket && !this.socket.destroyed) {
      this.socket.write(`SUB ${subject} ${sid}\r\n`);
    } else {
      this.connect();
    }

    return () => {
      this.subscriptions.delete(sid);
      if (this.connected && this.socket && !this.socket.destroyed) {
        this.socket.write(`UNSUB ${sid}\r\n`);
      }
    };
  }

  publish(subject, payload) {
    if (!this.url) return;
    assertSubject(subject, 'publish subject');
    this.queue.push({ subject, payload: payloadToBuffer(payload) });
    if (this.queue.length > MAX_QUEUE_DEPTH) {
      this.queue.splice(0, this.queue.length - MAX_QUEUE_DEPTH);
    }
    this.flush();
  }

  destroy() {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    this.socket?.destroy();
    this.socket = null;
    this.connecting = false;
    this.connected = false;
    this.buffer = Buffer.alloc(0);
  }

  connect() {
    if (this.connecting || this.connected || !this.url) return;
    const parsed = parseNatsUrl(this.url);
    if (!parsed) {
      this.logger.warn(`[nats-client] invalid NATS_URL: ${this.url}`);
      return;
    }

    this.connecting = true;
    const socket = net.createConnection(parsed, () => {
      this.connecting = false;
      this.connected = true;
      socket.write(
        'CONNECT {"verbose":false,"pedantic":false,"lang":"node","version":"dd-gleamlang-ws"}\r\n',
      );
      for (const [sid, subscription] of this.subscriptions) {
        socket.write(`SUB ${subscription.subject} ${sid}\r\n`);
      }
      this.flush();
    });
    this.socket = socket;

    socket.on('data', (chunk) => {
      this.buffer = Buffer.concat([this.buffer, chunk]);
      this.drain(socket);
    });
    socket.on('error', (error) => {
      this.logger.warn(`[nats-client] ${error.message}`);
    });
    socket.on('close', () => {
      if (this.socket === socket) {
        this.socket = null;
      }
      this.connecting = false;
      this.connected = false;
      this.buffer = Buffer.alloc(0);
      this.scheduleReconnect();
    });
  }

  flush() {
    if (!this.url) return;
    if (!this.connected || !this.socket || this.socket.destroyed) {
      this.connect();
      return;
    }

    while (this.queue.length > 0) {
      const next = this.queue.shift();
      if (!next) return;
      this.socket.write(
        Buffer.concat([
          Buffer.from(`PUB ${next.subject} ${next.payload.length}\r\n`, 'utf8'),
          next.payload,
          Buffer.from('\r\n', 'utf8'),
        ]),
      );
    }
  }

  drain(socket) {
    for (;;) {
      if (this.buffer.length === 0) return;

      if (startsWithAscii(this.buffer, 'MSG ')) {
        if (!this.drainMessage()) return;
        continue;
      }

      const lineEnd = this.buffer.indexOf('\r\n');
      if (lineEnd === -1) return;
      const line = this.buffer.subarray(0, lineEnd).toString('utf8');
      this.buffer = this.buffer.subarray(lineEnd + 2);

      if (line === 'PING') {
        socket.write('PONG\r\n');
      } else if (line.startsWith('-ERR')) {
        this.logger.warn(`[nats-client] server error: ${line}`);
      }
    }
  }

  drainMessage() {
    const headerEnd = this.buffer.indexOf('\r\n');
    if (headerEnd === -1) return false;

    const header = this.buffer.subarray(0, headerEnd).toString('utf8').split(/\s+/);
    const sid = Number(header[2]);
    const byteCount = Number(header[header.length - 1]);
    if (!Number.isFinite(sid) || !Number.isFinite(byteCount) || byteCount < 0) {
      this.buffer = this.buffer.subarray(headerEnd + 2);
      return true;
    }

    const payloadStart = headerEnd + 2;
    const frameEnd = payloadStart + byteCount + 2;
    if (this.buffer.length < frameEnd) return false;

    const payload = this.buffer.subarray(payloadStart, payloadStart + byteCount);
    this.buffer = this.buffer.subarray(frameEnd);
    const subscription = this.subscriptions.get(sid);
    if (subscription) {
      try {
        subscription.handler(payload.toString('utf8'));
      } catch (error) {
        this.logger.warn(
          `[nats-client] subscription handler failed: ${
            error instanceof Error ? error.message : String(error)
          }`,
        );
      }
    }
    return true;
  }

  scheduleReconnect() {
    if (this.reconnectTimer || !this.url) return;
    if (this.subscriptions.size === 0 && this.queue.length === 0) return;
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.connect();
    }, this.reconnectMs);
  }
}

function parseNatsUrl(raw) {
  try {
    const url = new URL(raw);
    if (url.protocol !== 'nats:') return null;
    return {
      host: url.hostname,
      port: url.port ? Number(url.port) : 4222,
    };
  } catch {
    return null;
  }
}

function payloadToBuffer(payload) {
  if (Buffer.isBuffer(payload)) return payload;
  if (payload instanceof Uint8Array) return Buffer.from(payload);
  if (typeof payload === 'string') return Buffer.from(payload, 'utf8');
  return Buffer.from(JSON.stringify(payload), 'utf8');
}

function startsWithAscii(buffer, prefix) {
  return (
    buffer.length >= prefix.length &&
    buffer.subarray(0, prefix.length).toString('ascii') === prefix
  );
}

function assertSubject(subject, label) {
  if (typeof subject !== 'string' || !/^[^\s]+$/.test(subject)) {
    throw new Error(`invalid NATS ${label}: ${subject}`);
  }
}
