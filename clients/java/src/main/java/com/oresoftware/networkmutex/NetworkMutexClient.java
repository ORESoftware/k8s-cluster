package com.oresoftware.networkmutex;

import com.oresoftware.networkmutex.Protocol.RequestType;
import com.oresoftware.networkmutex.Protocol.Response;
import com.oresoftware.networkmutex.Protocol.ResponseType;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.UUID;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.LinkedBlockingQueue;
import java.util.concurrent.TimeUnit;

/**
 * Multiplexed TCP client for {@code dd-rust-network-mutex}.
 *
 * <p>One instance is safe to share across threads: writes are serialized and a
 * background reader thread fans broker frames out to per-request queues keyed by
 * the correlation uuid (mirrors the Go client's {@code map[string]chan Response}).
 */
public final class NetworkMutexClient implements AutoCloseable {

  public static final class NetworkMutexException extends RuntimeException {
    public NetworkMutexException(String message) {
      super(message);
    }
  }

  public record SingleLockHandle(String key, String lockUuid, long fencingToken) {}

  public record CompositeLockHandle(List<String> keys, String lockUuid, Map<String, Long> fencingTokens) {}

  private final Socket socket;
  private final OutputStream out;
  private final Object writeLock = new Object();
  private final ConcurrentHashMap<String, LinkedBlockingQueue<Response>> inflight = new ConcurrentHashMap<>();
  private final Thread reader;
  private volatile boolean closed = false;
  private volatile String readError = null;

  private static final Response SENTINEL = new Response(Map.of("type", "ok", "uuid", "__sentinel__"));

  private NetworkMutexClient(Socket socket) throws IOException {
    this.socket = socket;
    this.out = socket.getOutputStream();
    this.reader = new Thread(this::readLoop, "network-mutex-reader");
    this.reader.setDaemon(true);
    this.reader.start();
  }

  public static NetworkMutexClient connect(String host, int port) {
    return connect(host, port, null, 5000);
  }

  public static NetworkMutexClient connect(String host, int port, String token, int connectTimeoutMs) {
    try {
      Socket socket = new Socket();
      socket.connect(new InetSocketAddress(host, port), connectTimeoutMs);
      socket.setTcpNoDelay(true);
      NetworkMutexClient client = new NetworkMutexClient(socket);
      if (token != null && !token.isEmpty()) {
        String uuid = newUuid();
        Response r = client.roundtrip(Protocol.authRequest(uuid, token), uuid, 30_000);
        if (r.type != ResponseType.AUTH || !r.bool("ok")) {
          client.close();
          throw new NetworkMutexException("auth rejected: " + r.error());
        }
      }
      return client;
    } catch (IOException e) {
      throw new NetworkMutexException("connect failed: " + e.getMessage());
    }
  }

  private static String newUuid() {
    return UUID.randomUUID().toString();
  }

  // ---- exclusive / composite ---------------------------------------------

  public SingleLockHandle acquire(String key, long ttlMs) {
    return acquire(key, ttlMs, null, 30_000);
  }

  public SingleLockHandle acquire(String key, long ttlMs, Integer maxHolders, long timeoutMs) {
    String uuid = newUuid();
    Response r = roundtripGrant(Protocol.lockRequestSingle(uuid, key, ttlMs, maxHolders, Boolean.TRUE), uuid, timeoutMs);
    if (r.type != ResponseType.LOCK || !r.acquired() || r.lockUuid().isEmpty()) {
      throw new NetworkMutexException("lock(" + key + ") failed: " + Json.stringify(r.raw));
    }
    return new SingleLockHandle(key, r.lockUuid(), r.fencingToken());
  }

  public Optional<SingleLockHandle> tryAcquire(String key, long ttlMs) {
    return tryAcquire(key, ttlMs, null, 30_000);
  }

  public Optional<SingleLockHandle> tryAcquire(String key, long ttlMs, Integer maxHolders, long timeoutMs) {
    String uuid = newUuid();
    Response r = roundtrip(Protocol.lockRequestSingle(uuid, key, ttlMs, maxHolders, Boolean.FALSE), uuid, timeoutMs);
    if (r.type == ResponseType.ERROR) {
      throw new NetworkMutexException("tryAcquire(" + key + ") error: " + r.error());
    }
    if (r.type != ResponseType.LOCK) {
      throw new NetworkMutexException("tryAcquire(" + key + ") unexpected: " + Json.stringify(r.raw));
    }
    if (!r.acquired() || r.lockUuid().isEmpty()) return Optional.empty();
    return Optional.of(new SingleLockHandle(key, r.lockUuid(), r.fencingToken()));
  }

  public CompositeLockHandle acquireMany(List<String> keys, long ttlMs) {
    String uuid = newUuid();
    Response r = roundtripGrant(Protocol.lockRequestComposite(uuid, keys, ttlMs, Boolean.TRUE), uuid, 30_000);
    if (r.type != ResponseType.COMPOSITE_LOCK || !r.acquired() || r.lockUuid().isEmpty()) {
      throw new NetworkMutexException("acquireMany failed: " + Json.stringify(r.raw));
    }
    return new CompositeLockHandle(keys, r.lockUuid(), r.fencingTokens());
  }

  public Optional<CompositeLockHandle> tryAcquireMany(List<String> keys, long ttlMs) {
    String uuid = newUuid();
    Response r = roundtrip(Protocol.lockRequestComposite(uuid, keys, ttlMs, Boolean.FALSE), uuid, 30_000);
    if (r.type == ResponseType.ERROR) {
      throw new NetworkMutexException("tryAcquireMany error: " + r.error());
    }
    if (r.type != ResponseType.COMPOSITE_LOCK) {
      throw new NetworkMutexException("tryAcquireMany unexpected: " + Json.stringify(r.raw));
    }
    if (!r.acquired() || r.lockUuid().isEmpty()) return Optional.empty();
    return Optional.of(new CompositeLockHandle(keys, r.lockUuid(), r.fencingTokens()));
  }

  public void release(SingleLockHandle h) {
    String uuid = newUuid();
    Response r = roundtrip(Protocol.unlockRequestSingle(uuid, h.key(), h.lockUuid(), false), uuid, 30_000);
    if (r.type != ResponseType.UNLOCK || !r.unlocked()) {
      throw new NetworkMutexException("unlock failed: " + Json.stringify(r.raw));
    }
  }

  public void release(CompositeLockHandle h) {
    String uuid = newUuid();
    Response r = roundtrip(Protocol.unlockRequestComposite(uuid, h.keys(), h.lockUuid()), uuid, 30_000);
    if (r.type != ResponseType.UNLOCK || !r.unlocked()) {
      throw new NetworkMutexException("unlock composite failed: " + Json.stringify(r.raw));
    }
  }

  // ---- reader / writer ----------------------------------------------------

  public SingleLockHandle acquireRead(String key) {
    String uuid = newUuid();
    Response r = roundtripUntilGranted(Protocol.rwRequest(RequestType.REGISTER_READ, uuid, key), uuid, 30_000);
    return new SingleLockHandle(key, r.lockUuid(), r.fencingToken());
  }

  public SingleLockHandle acquireWrite(String key) {
    String uuid = newUuid();
    Response r = roundtripUntilGranted(Protocol.rwRequest(RequestType.REGISTER_WRITE, uuid, key), uuid, 30_000);
    return new SingleLockHandle(key, r.lockUuid(), r.fencingToken());
  }

  public void releaseRead(String key) {
    String uuid = newUuid();
    roundtrip(Protocol.rwRequest(RequestType.END_READ, uuid, key), uuid, 30_000);
  }

  public void releaseWrite(String key) {
    String uuid = newUuid();
    roundtrip(Protocol.rwRequest(RequestType.END_WRITE, uuid, key), uuid, 30_000);
  }

  // ---- introspection ------------------------------------------------------

  public List<String> ls() {
    String uuid = newUuid();
    return roundtrip(Protocol.lsRequest(uuid), uuid, 30_000).keys();
  }

  public Response lockInfo(String key) {
    String uuid = newUuid();
    return roundtrip(Protocol.lockInfoRequest(uuid, key), uuid, 30_000);
  }

  // ---- lifecycle ----------------------------------------------------------

  @Override
  public void close() {
    if (closed) return;
    closed = true;
    try {
      socket.close();
    } catch (IOException ignored) {
      // best effort
    }
    for (var q : inflight.values()) {
      q.offer(SENTINEL);
    }
    inflight.clear();
  }

  // ---- internals ----------------------------------------------------------

  private LinkedBlockingQueue<Response> register(String uuid) {
    if (closed) throw new NetworkMutexException("client closed");
    var q = new LinkedBlockingQueue<Response>();
    inflight.put(uuid, q);
    return q;
  }

  private void send(String frame) {
    synchronized (writeLock) {
      try {
        out.write(frame.getBytes(StandardCharsets.UTF_8));
        out.flush();
      } catch (IOException e) {
        throw new NetworkMutexException("send failed: " + e.getMessage());
      }
    }
  }

  private Response next(LinkedBlockingQueue<Response> q, long timeoutMs) {
    try {
      Response r = q.poll(timeoutMs, TimeUnit.MILLISECONDS);
      if (r == null) throw new NetworkMutexException("timed out waiting for broker response");
      if (r == SENTINEL) {
        throw new NetworkMutexException(readError != null ? readError : "client closed");
      }
      return r;
    } catch (InterruptedException e) {
      Thread.currentThread().interrupt();
      throw new NetworkMutexException("interrupted");
    }
  }

  private Response roundtrip(String frame, String uuid, long timeoutMs) {
    var q = register(uuid);
    try {
      send(frame);
      return next(q, timeoutMs);
    } finally {
      inflight.remove(uuid);
    }
  }

  private Response roundtripGrant(String frame, String uuid, long timeoutMs) {
    var q = register(uuid);
    try {
      send(frame);
      while (true) {
        Response r = next(q, timeoutMs);
        if (r.type == ResponseType.ERROR) return r;
        if (r.type == ResponseType.LOCK || r.type == ResponseType.COMPOSITE_LOCK) {
          if (r.acquired() || r.has("error")) return r;
          continue; // queued notice
        }
        return r;
      }
    } finally {
      inflight.remove(uuid);
    }
  }

  private Response roundtripUntilGranted(String frame, String uuid, long timeoutMs) {
    var q = register(uuid);
    try {
      send(frame);
      while (true) {
        Response r = next(q, timeoutMs);
        if (r.granted()) return r;
        if (r.type == ResponseType.ERROR) {
          throw new NetworkMutexException("rw acquire failed: " + r.error());
        }
      }
    } finally {
      inflight.remove(uuid);
    }
  }

  private void readLoop() {
    try {
      InputStream in = socket.getInputStream();
      byte[] buf = new byte[65536];
      StringBuilder acc = new StringBuilder();
      int n;
      while (!closed && (n = in.read(buf)) > 0) {
        acc.append(new String(buf, 0, n, StandardCharsets.UTF_8));
        int nl;
        while ((nl = acc.indexOf("\n")) >= 0) {
          String line = acc.substring(0, nl);
          acc.delete(0, nl + 1);
          if (line.isBlank()) continue;
          dispatch(Response.parse(line));
        }
      }
    } catch (IOException e) {
      if (!closed) readError = e.getMessage();
    } catch (RuntimeException e) {
      if (!closed) readError = e.getMessage();
    } finally {
      close();
    }
  }

  private void dispatch(Response r) {
    var q = inflight.get(r.uuid);
    if (q != null) q.offer(r);
  }
}
