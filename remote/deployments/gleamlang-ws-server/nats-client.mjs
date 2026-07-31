import net from 'node:net';

const DEFAULT_RECONNECT_MS = 1000;
const MAX_RECONNECT_MS = 30_000;
const DEFAULT_CONNECT_TIMEOUT_MS = 5000;
const MAX_QUEUE_DEPTH = 500;
const MAX_QUEUE_BYTES = 8 * 1024 * 1024;
const MAX_PAYLOAD_BYTES = 1024 * 1024;
const MAX_INBOUND_BUFFER_BYTES = 2 * 1024 * 1024;
const MAX_CONTROL_LINE_BYTES = 4096;

let singleton = null;

export function getNatsClient(options = {}) {
  const url = options.url ?? process.env.NATS_URL ?? null;
  if (!singleton || singleton.url !== url) {
    singleton?.destroy();
    singleton = new NatsClient({
      url,
      logger: options.logger ?? console,
      reconnectMs: options.reconnectMs ?? DEFAULT_RECONNECT_MS,
      connectTimeoutMs: options.connectTimeoutMs ?? DEFAULT_CONNECT_TIMEOUT_MS,
    });
  }
  return singleton;
}

export class NatsClient {
  constructor({
    url,
    logger = console,
    reconnectMs = DEFAULT_RECONNECT_MS,
    connectTimeoutMs = DEFAULT_CONNECT_TIMEOUT_MS,
    maxQueueDepth = MAX_QUEUE_DEPTH,
    maxQueueBytes = MAX_QUEUE_BYTES,
    maxPayloadBytes = MAX_PAYLOAD_BYTES,
    maxInboundBufferBytes = MAX_INBOUND_BUFFER_BYTES,
  }) {
    this.url = url;
    this.logger = logger;
    this.reconnectMs = reconnectMs;
    this.connectTimeoutMs = connectTimeoutMs;
    this.maxQueueDepth = maxQueueDepth;
    this.maxQueueBytes = maxQueueBytes;
    this.maxPayloadBytes = maxPayloadBytes;
    this.maxInboundBufferBytes = maxInboundBufferBytes;
    this.socket = null;
    this.connecting = false;
    this.connected = false;
    this.destroyed = false;
    this.waitingForDrain = false;
    this.reconnectTimer = null;
    this.reconnectAttempts = 0;
    this.buffer = Buffer.alloc(0);
    this.queue = [];
    this.queueBytes = 0;
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
    if (!this.url || this.destroyed) return false;
    assertSubject(subject, 'publish subject', false);
    const payloadBuffer = payloadToBuffer(payload);
    if (payloadBuffer.length > this.maxPayloadBytes) {
      throw new Error(
        `NATS publish payload exceeds ${this.maxPayloadBytes} byte limit`,
      );
    }
    this.queue.push({ subject, payload: payloadBuffer });
    this.queueBytes += payloadBuffer.length;
    let dropped = 0;
    while (
      this.queue.length > this.maxQueueDepth ||
      this.queueBytes > this.maxQueueBytes
    ) {
      const oldest = this.queue.shift();
      if (!oldest) break;
      this.queueBytes -= oldest.payload.length;
      dropped += 1;
    }
    if (dropped > 0) {
      this.logger.warn(
        `[nats-client] outbound queue full; dropped ${dropped} oldest message(s)`,
      );
    }
    this.flush();
    return true;
  }

  destroy() {
    this.destroyed = true;
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    this.socket?.destroy();
    this.socket = null;
    this.connecting = false;
    this.connected = false;
    this.waitingForDrain = false;
    this.buffer = Buffer.alloc(0);
    this.queue = [];
    this.queueBytes = 0;
  }

  connect() {
    if (this.destroyed || this.connecting || this.connected || !this.url) return;
    const parsed = parseNatsUrl(this.url);
    if (!parsed) {
      // Never echo a URL: it may contain a NATS user, password, or token.
      this.logger.warn('[nats-client] invalid NATS_URL');
      return;
    }

    this.connecting = true;
    const socket = net.createConnection({ host: parsed.host, port: parsed.port }, () => {
      socket.setTimeout(0);
      socket.setNoDelay(true);
      this.connecting = false;
      this.connected = true;
      this.reconnectAttempts = 0;
      const connectPayload = {
        verbose: false,
        pedantic: true,
        lang: 'node',
        version: 'dd-gleamlang-ws',
      };
      if (parsed.authToken) {
        connectPayload.auth_token = parsed.authToken;
      } else if (parsed.user) {
        connectPayload.user = parsed.user;
        connectPayload.pass = parsed.password;
      }
      socket.write(`CONNECT ${JSON.stringify(connectPayload)}\r\n`);
      for (const [sid, subscription] of this.subscriptions) {
        socket.write(`SUB ${subscription.subject} ${sid}\r\n`);
      }
      this.flush();
    });
    this.socket = socket;
    socket.setTimeout(this.connectTimeoutMs, () => {
      this.logger.warn('[nats-client] connect timeout');
      socket.destroy();
    });

    socket.on('data', (chunk) => {
      if (this.buffer.length + chunk.length > this.maxInboundBufferBytes) {
        this.protocolError('inbound buffer limit exceeded');
        return;
      }
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
      this.waitingForDrain = false;
      this.buffer = Buffer.alloc(0);
      this.scheduleReconnect();
    });
  }

  flush() {
    if (!this.url || this.destroyed || this.waitingForDrain) return;
    if (!this.connected || !this.socket || this.socket.destroyed) {
      this.connect();
      return;
    }

    while (this.queue.length > 0) {
      const next = this.queue.shift();
      if (!next) return;
      this.queueBytes -= next.payload.length;
      const writable = this.socket.write(
        Buffer.concat([
          Buffer.from(`PUB ${next.subject} ${next.payload.length}\r\n`, 'utf8'),
          next.payload,
          Buffer.from('\r\n', 'utf8'),
        ]),
      );
      if (!writable) {
        const socket = this.socket;
        this.waitingForDrain = true;
        socket.once('drain', () => {
          if (this.socket !== socket || socket.destroyed) return;
          this.waitingForDrain = false;
          this.flush();
        });
        return;
      }
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
      if (lineEnd === -1) {
        if (this.buffer.length > MAX_CONTROL_LINE_BYTES) {
          this.protocolError('control line limit exceeded');
        }
        return;
      }
      if (lineEnd > MAX_CONTROL_LINE_BYTES) {
        this.protocolError('control line limit exceeded');
        return;
      }
      const line = this.buffer.subarray(0, lineEnd).toString('utf8');
      this.buffer = this.buffer.subarray(lineEnd + 2);

      if (line === 'PING') {
        socket.write('PONG\r\n');
      } else if (line.startsWith('-ERR')) {
        this.logger.warn(`[nats-client] server error: ${line}`);
      } else if (
        line !== 'PONG' &&
        line !== '+OK' &&
        !line.startsWith('INFO ')
      ) {
        this.protocolError('unexpected protocol line');
        return;
      }
    }
  }

  drainMessage() {
    const headerEnd = this.buffer.indexOf('\r\n');
    if (headerEnd === -1) return false;

    const header = this.buffer.subarray(0, headerEnd).toString('utf8').split(/\s+/);
    const subject = header[1];
    const sid = Number(header[2]);
    const byteCount = Number(header[header.length - 1]);
    if (
      (header.length !== 4 && header.length !== 5) ||
      !validSubject(subject, true) ||
      !Number.isSafeInteger(sid) ||
      sid <= 0 ||
      !Number.isSafeInteger(byteCount) ||
      byteCount < 0 ||
      byteCount > this.maxPayloadBytes
    ) {
      this.protocolError('invalid MSG frame');
      return false;
    }

    const payloadStart = headerEnd + 2;
    const frameEnd = payloadStart + byteCount + 2;
    if (this.buffer.length < frameEnd) return false;
    if (this.buffer[frameEnd - 2] !== 13 || this.buffer[frameEnd - 1] !== 10) {
      this.protocolError('invalid MSG terminator');
      return false;
    }

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
    if (this.destroyed || this.reconnectTimer || !this.url) return;
    if (this.subscriptions.size === 0 && this.queue.length === 0) return;
    const exponential = Math.min(
      this.reconnectMs * 2 ** this.reconnectAttempts,
      MAX_RECONNECT_MS,
    );
    const delay = Math.max(1, Math.round(exponential * (0.8 + Math.random() * 0.4)));
    this.reconnectAttempts += 1;
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.connect();
    }, delay);
  }

  protocolError(reason) {
    this.logger.warn(`[nats-client] protocol error: ${reason}`);
    this.socket?.destroy();
  }
}

function parseNatsUrl(raw) {
  try {
    const url = new URL(raw);
    if (
      url.protocol !== 'nats:' ||
      !url.hostname ||
      (url.pathname !== '' && url.pathname !== '/') ||
      url.search ||
      url.hash
    ) {
      return null;
    }
    const port = url.port ? Number(url.port) : 4222;
    if (!Number.isSafeInteger(port) || port < 1 || port > 65_535) return null;
    const user = decodeURIComponent(url.username);
    const password = decodeURIComponent(url.password);
    if (!user && password) return null;
    return {
      host: url.hostname,
      port,
      user: password ? user : null,
      password: password || null,
      authToken: user && !password ? user : null,
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

function assertSubject(subject, label, allowWildcards = true) {
  if (!validSubject(subject, allowWildcards)) {
    throw new Error(`invalid NATS ${label}: ${subject}`);
  }
}

function validSubject(subject, allowWildcards) {
  if (
    typeof subject !== 'string' ||
    subject.length === 0 ||
    subject.length > 255 ||
    /[\s\u0000]/.test(subject) ||
    subject.startsWith('.') ||
    subject.endsWith('.') ||
    subject.includes('..')
  ) {
    return false;
  }
  const tokens = subject.split('.');
  return tokens.every((token, index) => {
    if (!token) return false;
    if (token === '*') return allowWildcards;
    if (token === '>') return allowWildcards && index === tokens.length - 1;
    return !token.includes('*') && !token.includes('>');
  });
}
