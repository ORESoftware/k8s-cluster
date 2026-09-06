package com.oresoftware.networkmutex;

import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * Wire protocol for {@code dd-rust-network-mutex} (Java mirror of {@code src/protocol.rs}).
 *
 * <p>camelCase, newline-delimited JSON. The {@code type} discriminator is a Java
 * {@code enum} with a {@code switch} over every variant rather than the bare-string
 * {@code if (data.type === "...")} chains the upstream Node library uses. See
 * {@code ../../PROTOCOL.md} for the single source of truth.
 */
public final class Protocol {

  private Protocol() {}

  public static final int MAX_COMPOSITE_KEYS = 5;
  public static final String PROTOCOL_VERSION = "0.1.0";

  public enum RequestType {
    VERSION("version"),
    AUTH("auth"),
    LOCK("lock"),
    UNLOCK("unlock"),
    REGISTER_READ("registerRead"),
    REGISTER_WRITE("registerWrite"),
    END_READ("endRead"),
    END_WRITE("endWrite"),
    LOCK_INFO("lockInfo"),
    LS("ls"),
    HEARTBEAT("heartbeat");

    public final String wire;

    RequestType(String wire) {
      this.wire = wire;
    }
  }

  public enum ResponseType {
    VERSION("version"),
    AUTH("auth"),
    LOCK("lock"),
    COMPOSITE_LOCK("compositeLock"),
    UNLOCK("unlock"),
    REGISTER_READ_RESULT("registerReadResult"),
    REGISTER_WRITE_RESULT("registerWriteResult"),
    END_READ_RESULT("endReadResult"),
    END_WRITE_RESULT("endWriteResult"),
    LOCK_INFO("lockInfo"),
    LS_RESULT("lsResult"),
    REELECTION("reelection"),
    ERROR("error"),
    OK("ok"),
    UNKNOWN("");

    public final String wire;

    ResponseType(String wire) {
      this.wire = wire;
    }

    public static ResponseType fromWire(String s) {
      for (ResponseType t : values()) {
        if (t.wire.equals(s)) return t;
      }
      return UNKNOWN;
    }
  }

  // ---- request builders -> one newline-terminated JSON frame --------------

  private static String frame(Map<String, Object> obj) {
    return Json.stringify(obj) + "\n";
  }

  public static String versionRequest(String uuid, String value) {
    var o = new LinkedHashMap<String, Object>();
    o.put("type", RequestType.VERSION.wire);
    o.put("uuid", uuid);
    o.put("value", value);
    return frame(o);
  }

  public static String authRequest(String uuid, String token) {
    var o = new LinkedHashMap<String, Object>();
    o.put("type", RequestType.AUTH.wire);
    o.put("uuid", uuid);
    o.put("token", token);
    return frame(o);
  }

  public static String lockRequestSingle(String uuid, String key, long ttlMs, Integer maxHolders) {
    return lockRequestSingle(uuid, key, ttlMs, maxHolders, null);
  }

  public static String lockRequestSingle(String uuid, String key, long ttlMs, Integer maxHolders, Boolean wait) {
    var o = new LinkedHashMap<String, Object>();
    o.put("type", RequestType.LOCK.wire);
    o.put("uuid", uuid);
    o.put("key", key);
    if (ttlMs > 0) o.put("ttl", ttlMs);
    if (maxHolders != null) o.put("max", (long) maxHolders);
    if (wait != null) o.put("wait", wait);
    return frame(o);
  }

  public static String lockRequestComposite(String uuid, List<String> keys, long ttlMs) {
    return lockRequestComposite(uuid, keys, ttlMs, null);
  }

  public static String lockRequestComposite(String uuid, List<String> keys, long ttlMs, Boolean wait) {
    if (keys.isEmpty() || keys.size() > MAX_COMPOSITE_KEYS) {
      throw new IllegalArgumentException("composite key count must be 1..=5, got " + keys.size());
    }
    var o = new LinkedHashMap<String, Object>();
    o.put("type", RequestType.LOCK.wire);
    o.put("uuid", uuid);
    o.put("keys", keys);
    if (ttlMs > 0) o.put("ttl", ttlMs);
    if (wait != null) o.put("wait", wait);
    return frame(o);
  }

  public static String unlockRequestSingle(String uuid, String key, String lockUuid, boolean force) {
    var o = new LinkedHashMap<String, Object>();
    o.put("type", RequestType.UNLOCK.wire);
    o.put("uuid", uuid);
    o.put("key", key);
    if (lockUuid != null && !lockUuid.isEmpty()) o.put("lockUuid", lockUuid);
    if (force) o.put("force", Boolean.TRUE);
    return frame(o);
  }

  public static String unlockRequestComposite(String uuid, List<String> keys, String lockUuid) {
    var o = new LinkedHashMap<String, Object>();
    o.put("type", RequestType.UNLOCK.wire);
    o.put("uuid", uuid);
    o.put("keys", keys);
    if (lockUuid != null && !lockUuid.isEmpty()) o.put("lockUuid", lockUuid);
    return frame(o);
  }

  public static String rwRequest(RequestType type, String uuid, String key) {
    var o = new LinkedHashMap<String, Object>();
    o.put("type", type.wire);
    o.put("uuid", uuid);
    o.put("key", key);
    return frame(o);
  }

  public static String lockInfoRequest(String uuid, String key) {
    return rwRequest(RequestType.LOCK_INFO, uuid, key);
  }

  public static String lsRequest(String uuid) {
    var o = new LinkedHashMap<String, Object>();
    o.put("type", RequestType.LS.wire);
    o.put("uuid", uuid);
    return frame(o);
  }

  /** Parsed broker frame. */
  public static final class Response {
    public final ResponseType type;
    public final String uuid;
    public final Map<String, Object> raw;

    public Response(Map<String, Object> raw) {
      this.raw = raw;
      this.type = ResponseType.fromWire(str("type"));
      this.uuid = str("uuid");
    }

    public static Response parse(String line) {
      return new Response(Json.parseObject(line));
    }

    public String str(String key) {
      Object v = raw.get(key);
      return v instanceof String s ? s : "";
    }

    public boolean bool(String key) {
      Object v = raw.get(key);
      return v instanceof Boolean b && b;
    }

    public boolean has(String key) {
      return raw.containsKey(key) && raw.get(key) != null;
    }

    public long longOr(String key, long def) {
      Object v = raw.get(key);
      return v instanceof Number n ? n.longValue() : def;
    }

    public boolean acquired() {
      return bool("acquired");
    }

    public boolean unlocked() {
      return bool("unlocked");
    }

    public boolean granted() {
      return bool("granted");
    }

    public String lockUuid() {
      return str("lockUuid");
    }

    public String error() {
      return str("error");
    }

    public long fencingToken() {
      return longOr("fencingToken", 0);
    }

    @SuppressWarnings("unchecked")
    public List<String> keys() {
      Object v = raw.get("keys");
      return v instanceof List ? (List<String>) v : List.of();
    }

    public Map<String, Long> fencingTokens() {
      var out = new LinkedHashMap<String, Long>();
      Object v = raw.get("fencingTokens");
      if (v instanceof Map<?, ?> m) {
        for (Map.Entry<?, ?> e : m.entrySet()) {
          if (e.getValue() instanceof Number n) {
            out.put(String.valueOf(e.getKey()), n.longValue());
          }
        }
      }
      return out;
    }
  }
}
