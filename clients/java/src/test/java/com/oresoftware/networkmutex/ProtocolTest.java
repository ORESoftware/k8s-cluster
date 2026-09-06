package com.oresoftware.networkmutex;

import com.oresoftware.networkmutex.Protocol.Response;
import com.oresoftware.networkmutex.Protocol.ResponseType;

import java.util.List;
import java.util.Map;

/**
 * Offline protocol round-trip tests (no broker, no JUnit dependency).
 *
 * <pre>
 *   ./build.sh &amp;&amp; java -cp out com.oresoftware.networkmutex.ProtocolTest
 * </pre>
 */
public final class ProtocolTest {

  private static int failures = 0;

  private static void check(boolean cond, String name) {
    if (!cond) {
      System.err.println("FAIL: " + name);
      failures++;
    }
  }

  public static void main(String[] args) {
    // Request: camelCase wire fields, newline-terminated.
    String f = Protocol.lockRequestSingle("u-1", "k1", 4000, 1);
    check(f.endsWith("\n"), "frame newline-terminated");
    Map<String, Object> v = Json.parseObject(f);
    check("lock".equals(v.get("type")), "lock type");
    check("k1".equals(v.get("key")), "key field");
    check(((Number) v.get("ttl")).longValue() == 4000, "ttl field");
    check(((Number) v.get("max")).longValue() == 1, "max field");
    check(!v.containsKey("keys"), "no keys field for single");

    Map<String, Object> waitTrue =
        Json.parseObject(Protocol.lockRequestSingle("u-wait", "k1", 4000, null, Boolean.TRUE));
    check(Boolean.TRUE.equals(waitTrue.get("wait")), "single wait true preserved");
    check(!v.containsKey("wait"), "wait omitted by default");

    // Composite request keeps keys unsorted (broker sorts).
    Map<String, Object> c = Json.parseObject(Protocol.lockRequestComposite("u-2", List.of("c", "a", "b"), 0, Boolean.FALSE));
    @SuppressWarnings("unchecked")
    List<String> keys = (List<String>) c.get("keys");
    check(keys.size() == 3 && keys.get(0).equals("c"), "composite keys preserved");
    check(Boolean.FALSE.equals(c.get("wait")), "composite wait false preserved");

    // Composite oversize rejected.
    boolean threw = false;
    try {
      Protocol.lockRequestComposite("u", List.of("a", "b", "c", "d", "e", "f"), 0);
    } catch (IllegalArgumentException e) {
      threw = true;
    }
    check(threw, "oversize composite rejected");

    // Response: composite grant with 64-bit token precision.
    Response r = Response.parse(
        "{\"type\":\"compositeLock\",\"uuid\":\"u-1\",\"keys\":[\"a\",\"b\"],\"acquired\":true,"
            + "\"lockUuid\":\"L-1\",\"fencingTokens\":{\"a\":1780240060223,\"b\":12}}");
    check(r.type == ResponseType.COMPOSITE_LOCK, "composite response type");
    check(r.acquired(), "composite acquired");
    check(r.lockUuid().equals("L-1"), "composite lockUuid");
    check(r.fencingTokens().get("a") == 1780240060223L, "64-bit fencing token precision");

    // Response: unknown type degrades to UNKNOWN.
    Response u = Response.parse("{\"type\":\"totallyBogus\",\"uuid\":\"u\"}");
    check(u.type == ResponseType.UNKNOWN, "unknown response type");

    // Response: distinguishes absent from false.
    Response ok = Response.parse("{\"type\":\"ok\",\"uuid\":\"u\"}");
    check(!ok.has("acquired"), "absent acquired not present");

    if (failures == 0) {
      System.out.println("[test-java] all protocol tests passed");
    } else {
      System.err.println("[test-java] " + failures + " failure(s)");
      System.exit(1);
    }
  }
}
