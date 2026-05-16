import net from 'node:net';

const natsUrl = process.env.NATS_URL ?? 'nats://dd-nats.messaging.svc.cluster.local:4222';
const subject = process.env.NATS_EVENT_SUBJECT ?? 'dd.remote.events';
const broadcastUrl = process.env.GLEAM_BROADCAST_URL ?? 'http://127.0.0.1:8081/broadcast';
const broadcastSecret = requiredEnv('GLEAM_BROADCAST_SECRET');

let buffer = '';
let reconnectTimer = null;

function requiredEnv(name) {
  const value = process.env[name];
  if (!value?.trim()) {
    throw new Error(`${name} must be configured`);
  }
  return value;
}

function parseNatsUrl(raw) {
  const url = new URL(raw);
  return {
    host: url.hostname,
    port: url.port ? Number(url.port) : 4222,
  };
}

function connect() {
  const target = parseNatsUrl(natsUrl);
  const socket = net.createConnection(target, () => {
    socket.write(
      'CONNECT {"verbose":false,"pedantic":false,"lang":"node","version":"dd-gleam-nats-bridge"}\r\n',
    );
    socket.write(`SUB ${subject} 1\r\n`);
    console.info(`[nats-bridge] subscribed ${subject}`);
  });

  socket.on('data', (chunk) => {
    buffer += chunk.toString('utf8');
    drain(socket);
  });
  socket.on('error', (error) => {
    console.warn(`[nats-bridge] ${error.message}`);
  });
  socket.on('close', () => {
    scheduleReconnect();
  });
}

function drain(socket) {
  for (;;) {
    if (buffer.startsWith('PING')) {
      socket.write('PONG\r\n');
      buffer = buffer.slice('PING\r\n'.length);
      continue;
    }

    if (buffer.startsWith('MSG ')) {
      const headerEnd = buffer.indexOf('\r\n');
      if (headerEnd === -1) {
        return;
      }
      const header = buffer.slice(0, headerEnd).split(/\s+/);
      const byteCount = Number(header[header.length - 1]);
      if (!Number.isFinite(byteCount)) {
        buffer = buffer.slice(headerEnd + 2);
        continue;
      }
      const payloadStart = headerEnd + 2;
      const frameEnd = payloadStart + byteCount + 2;
      if (buffer.length < frameEnd) {
        return;
      }
      const payload = buffer.slice(payloadStart, payloadStart + byteCount);
      buffer = buffer.slice(frameEnd);
      void broadcast(payload);
      continue;
    }

    const lineEnd = buffer.indexOf('\r\n');
    if (lineEnd === -1) {
      return;
    }
    const line = buffer.slice(0, lineEnd);
    buffer = buffer.slice(lineEnd + 2);
    if (line.startsWith('-ERR')) {
      console.warn(`[nats-bridge] ${line}`);
    }
  }
}

async function broadcast(payload) {
  try {
    const response = await fetch(broadcastUrl, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        'x-dd-internal-auth': broadcastSecret,
      },
      body: payload,
    });
    if (!response.ok) {
      console.warn(`[nats-bridge] broadcast failed ${response.status}`);
    }
  } catch (error) {
    console.warn(
      `[nats-bridge] broadcast error: ${error instanceof Error ? error.message : String(error)}`,
    );
  }
}

function scheduleReconnect() {
  if (reconnectTimer) {
    return;
  }
  reconnectTimer = setTimeout(() => {
    reconnectTimer = null;
    connect();
  }, 1000);
}

connect();
