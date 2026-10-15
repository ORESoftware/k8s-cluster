package com.oresoftware.networkmutex;

import com.oresoftware.networkmutex.NetworkMutexClient.CompositeLockHandle;
import com.oresoftware.networkmutex.NetworkMutexClient.SingleLockHandle;

import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Optional;

public final class CrossLanguageWorker {

  private static final String host = envOr("LIVE_MUTEX_HOST", "127.0.0.1");
  private static final int port = Integer.parseInt(envOr("LIVE_MUTEX_PORT", "6970"));
  private static final String lang = envOr("LMX_WORKER_LANG", "java");
  private static final String worker = envOr("LMX_WORKER_ID", lang + "-0");
  private static final long seed = Long.parseUnsignedLong(envOr("LMX_WORKER_SEED", "1"));
  private static final int ops = Integer.parseInt(envOr("LMX_WORKER_OPS", "50"));
  private static final String keyPrefix = envOr("LMX_FUZZ_KEY_PREFIX", "cross");
  private static final int keyCount = Integer.parseInt(envOr("LMX_FUZZ_KEY_COUNT", "5"));
  private static final long ttlMs = 60_000;

  private static final BufferedReader stdin =
      new BufferedReader(new InputStreamReader(System.in));

  private CrossLanguageWorker() {}

  private static String envOr(String key, String def) {
    String v = System.getenv(key);
    return (v == null || v.isEmpty()) ? def : v;
  }

  private static final class Rng {
    private long x;

    Rng(long seed) {
      x = seed ^ 0x9E37_79B9_7F4A_7C15L;
      if (x == 0) x = 1;
    }

    int below(int n) {
      x ^= x >>> 12;
      x ^= x << 25;
      x ^= x >>> 27;
      long y = x * 0x2545_F491_4F6C_DD1DL;
      return (int) Long.remainderUnsigned(y, n);
    }
  }

  private static void emit(Map<String, Object> event) throws Exception {
    System.out.println(Json.stringify(event));
    System.out.flush();
    String line = stdin.readLine();
    if (!"ack".equals(line == null ? "" : line.trim())) {
      throw new IllegalStateException("expected ack from harness, got " + line);
    }
  }

  private static List<String> chooseKeys(Rng rng, List<String> keys) {
    int want = 2 + rng.below(Math.min(3, keys.size() - 1));
    var pool = new ArrayList<>(keys);
    var out = new ArrayList<String>();
    for (int i = 0; i < want && !pool.isEmpty(); i++) {
      out.add(pool.remove(rng.below(pool.size())));
    }
    out.sort(String::compareTo);
    return new ArrayList<>(new LinkedHashSet<>(out));
  }

  private static void holdBriefly(Rng rng) throws InterruptedException {
    int ms = rng.below(4);
    if (ms > 0) Thread.sleep(ms);
  }

  private static void grantReleaseExclusive(
      NetworkMutexClient client,
      Object handle,
      Rng rng) throws Exception {
    String lockUuid;
    List<String> keys;
    Map<String, Long> tokens = new LinkedHashMap<>();
    if (handle instanceof SingleLockHandle h) {
      lockUuid = h.lockUuid();
      keys = List.of(h.key());
      tokens.put(h.key(), h.fencingToken());
    } else if (handle instanceof CompositeLockHandle h) {
      lockUuid = h.lockUuid();
      keys = h.keys();
      tokens.putAll(h.fencingTokens());
    } else {
      throw new IllegalArgumentException("unknown handle " + handle);
    }

    emit(event("grant", lockUuid, "exclusive", keys, tokens));
    holdBriefly(rng);
    emit(releaseEvent(lockUuid));
    if (handle instanceof SingleLockHandle h) {
      client.release(h);
    } else {
      client.release((CompositeLockHandle) handle);
    }
  }

  private static Map<String, Object> event(
      String event,
      String lockUuid,
      String kind,
      List<String> keys,
      Map<String, Long> tokens) {
    var out = new LinkedHashMap<String, Object>();
    out.put("event", event);
    out.put("lang", lang);
    out.put("worker", worker);
    out.put("lockUuid", lockUuid);
    out.put("kind", kind);
    out.put("keys", keys);
    out.put("tokens", tokens);
    return out;
  }

  private static Map<String, Object> releaseEvent(String lockUuid) {
    var out = new LinkedHashMap<String, Object>();
    out.put("event", "release");
    out.put("lang", lang);
    out.put("worker", worker);
    out.put("lockUuid", lockUuid);
    return out;
  }

  public static void main(String[] args) throws Exception {
    var rng = new Rng(seed);
    var keys = new ArrayList<String>();
    for (int i = 0; i < keyCount; i++) keys.add(keyPrefix + "-" + i);

    try (NetworkMutexClient client = NetworkMutexClient.connect(host, port)) {
      for (int op = 0; op < ops; op++) {
        int roll = rng.below(100);
        if (roll < 30) {
          Optional<SingleLockHandle> h = client.tryAcquire(keys.get(rng.below(keys.size())), ttlMs);
          if (h.isPresent()) grantReleaseExclusive(client, h.get(), rng);
        } else if (roll < 50) {
          grantReleaseExclusive(
              client,
              client.acquire(keys.get(rng.below(keys.size())), ttlMs),
              rng);
        } else if (roll < 65) {
          Optional<CompositeLockHandle> h = client.tryAcquireMany(chooseKeys(rng, keys), ttlMs);
          if (h.isPresent()) grantReleaseExclusive(client, h.get(), rng);
        } else if (roll < 75) {
          grantReleaseExclusive(client, client.acquireMany(chooseKeys(rng, keys), ttlMs), rng);
        } else if (roll < 92) {
          String key = keys.get(rng.below(keys.size()));
          SingleLockHandle h = client.acquireRead(key);
          emit(event("grant", h.lockUuid(), "read", List.of(key), Map.of(key, h.fencingToken())));
          holdBriefly(rng);
          emit(releaseEvent(h.lockUuid()));
          client.releaseRead(key);
        } else {
          String key = keys.get(rng.below(keys.size()));
          SingleLockHandle h = client.acquireWrite(key);
          emit(event("grant", h.lockUuid(), "write", List.of(key), Map.of(key, h.fencingToken())));
          holdBriefly(rng);
          emit(releaseEvent(h.lockUuid()));
          client.releaseWrite(key);
        }
      }
    }

    var done = new LinkedHashMap<String, Object>();
    done.put("event", "done");
    done.put("lang", lang);
    done.put("worker", worker);
    done.put("ops", (long) ops);
    System.out.println(Json.stringify(done));
    System.out.flush();
  }
}
