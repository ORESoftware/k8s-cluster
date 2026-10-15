package com.oresoftware.networkmutex;

import com.oresoftware.networkmutex.NetworkMutexClient.CompositeLockHandle;
import com.oresoftware.networkmutex.NetworkMutexClient.SingleLockHandle;

import java.util.List;
import java.util.concurrent.CountDownLatch;

/**
 * End-to-end smoke test mirroring clients/go/cmd/smoke/main.go.
 *
 * <pre>
 *   ./build.sh &amp;&amp; java -cp out com.oresoftware.networkmutex.Smoke
 * </pre>
 *
 * Override host/port via LIVE_MUTEX_HOST / LIVE_MUTEX_PORT.
 */
public final class Smoke {

  private static String envOr(String key, String def) {
    String v = System.getenv(key);
    return (v == null || v.isEmpty()) ? def : v;
  }

  public static void main(String[] args) throws Exception {
    String host = envOr("LIVE_MUTEX_HOST", "127.0.0.1");
    int port = Integer.parseInt(envOr("LIVE_MUTEX_PORT", "6970"));

    try (NetworkMutexClient client = NetworkMutexClient.connect(host, port)) {
      System.out.println("[smoke-java] connected " + host + ":" + port);

      SingleLockHandle ex = client.acquire("smoke-java-exclusive", 5000);
      System.out.println("[smoke-java] exclusive grant: lockUuid=" + ex.lockUuid()
          + " fencing=" + ex.fencingToken());
      client.release(ex);
      System.out.println("[smoke-java] released exclusive");

      CompositeLockHandle comp = client.acquireMany(
          List.of("smoke-java-a", "smoke-java-b", "smoke-java-c"), 5000);
      System.out.println("[smoke-java] composite grant: lockUuid=" + comp.lockUuid()
          + " tokens=" + comp.fencingTokens());
      client.release(comp);
      System.out.println("[smoke-java] released composite");

      SingleLockHandle w = client.acquireWrite("smoke-java-rw");
      System.out.println("[smoke-java] writer grant: id=" + w.lockUuid()
          + " fencing=" + w.fencingToken());
      client.releaseWrite("smoke-java-rw");

      long[] tokens = new long[2];
      CountDownLatch latch = new CountDownLatch(2);
      for (int i = 0; i < 2; i++) {
        final int idx = i;
        new Thread(() -> {
          SingleLockHandle r = client.acquireRead("smoke-java-rw");
          tokens[idx] = r.fencingToken();
          latch.countDown();
        }).start();
      }
      latch.await();
      System.out.println("[smoke-java] readers: " + tokens[0] + " " + tokens[1]);
      client.releaseRead("smoke-java-rw");
      client.releaseRead("smoke-java-rw");

      System.out.println("[smoke-java] OK");
    } catch (RuntimeException e) {
      System.err.println("[smoke-java] FAILED: " + e.getMessage());
      System.exit(1);
    }
  }
}
