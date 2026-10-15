package com.oresoftware.networkmutex;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * Minimal, dependency-free JSON for the network-mutex protocol.
 *
 * <p>The broker speaks flat-ish JSON objects (strings, numbers, bools, null,
 * arrays of strings, and string-&gt;number maps), so this is a compact
 * recursive-descent parser plus serializer rather than a general library. We
 * parse integer-looking numbers as {@link Long} so 64-bit fencing tokens keep
 * full precision instead of going through a double.
 */
public final class Json {

  private Json() {}

  // ---- serialize ----------------------------------------------------------

  public static String stringify(Object v) {
    StringBuilder sb = new StringBuilder();
    write(sb, v);
    return sb.toString();
  }

  private static void write(StringBuilder sb, Object v) {
    if (v == null) {
      sb.append("null");
    } else if (v instanceof String s) {
      writeString(sb, s);
    } else if (v instanceof Boolean b) {
      sb.append(b ? "true" : "false");
    } else if (v instanceof Number n) {
      sb.append(n.toString());
    } else if (v instanceof Map<?, ?> m) {
      sb.append('{');
      boolean first = true;
      for (Map.Entry<?, ?> e : m.entrySet()) {
        if (!first) sb.append(',');
        first = false;
        writeString(sb, String.valueOf(e.getKey()));
        sb.append(':');
        write(sb, e.getValue());
      }
      sb.append('}');
    } else if (v instanceof List<?> list) {
      sb.append('[');
      for (int i = 0; i < list.size(); i++) {
        if (i > 0) sb.append(',');
        write(sb, list.get(i));
      }
      sb.append(']');
    } else {
      throw new IllegalArgumentException("cannot serialize " + v.getClass());
    }
  }

  private static void writeString(StringBuilder sb, String s) {
    sb.append('"');
    for (int i = 0; i < s.length(); i++) {
      char c = s.charAt(i);
      switch (c) {
        case '"' -> sb.append("\\\"");
        case '\\' -> sb.append("\\\\");
        case '\n' -> sb.append("\\n");
        case '\r' -> sb.append("\\r");
        case '\t' -> sb.append("\\t");
        default -> {
          if (c < 0x20) {
            sb.append(String.format("\\u%04x", (int) c));
          } else {
            sb.append(c);
          }
        }
      }
    }
    sb.append('"');
  }

  // ---- parse --------------------------------------------------------------

  public static Object parse(String s) {
    Parser p = new Parser(s);
    Object v = p.parseValue();
    p.skipWs();
    return v;
  }

  @SuppressWarnings("unchecked")
  public static Map<String, Object> parseObject(String s) {
    Object v = parse(s);
    if (!(v instanceof Map)) {
      throw new IllegalArgumentException("expected a JSON object");
    }
    return (Map<String, Object>) v;
  }

  private static final class Parser {
    private final String s;
    private int i = 0;

    Parser(String s) {
      this.s = s;
    }

    void skipWs() {
      while (i < s.length()) {
        char c = s.charAt(i);
        if (c == ' ' || c == '\t' || c == '\n' || c == '\r') i++;
        else break;
      }
    }

    private char peek() {
      if (i >= s.length()) throw new IllegalStateException("unexpected end of JSON");
      return s.charAt(i);
    }

    Object parseValue() {
      skipWs();
      char c = peek();
      return switch (c) {
        case '{' -> parseObj();
        case '[' -> parseArr();
        case '"' -> parseStr();
        case 't', 'f' -> parseBool();
        case 'n' -> parseNull();
        default -> parseNumber();
      };
    }

    private Map<String, Object> parseObj() {
      Map<String, Object> obj = new LinkedHashMap<>();
      i++; // {
      skipWs();
      if (peek() == '}') {
        i++;
        return obj;
      }
      while (true) {
        skipWs();
        String key = parseStr();
        skipWs();
        if (peek() != ':') throw new IllegalStateException("expected ':'");
        i++;
        obj.put(key, parseValue());
        skipWs();
        char c = peek();
        if (c == ',') {
          i++;
          continue;
        }
        if (c == '}') {
          i++;
          break;
        }
        throw new IllegalStateException("expected ',' or '}'");
      }
      return obj;
    }

    private List<Object> parseArr() {
      List<Object> arr = new ArrayList<>();
      i++; // [
      skipWs();
      if (peek() == ']') {
        i++;
        return arr;
      }
      while (true) {
        arr.add(parseValue());
        skipWs();
        char c = peek();
        if (c == ',') {
          i++;
          continue;
        }
        if (c == ']') {
          i++;
          break;
        }
        throw new IllegalStateException("expected ',' or ']'");
      }
      return arr;
    }

    private String parseStr() {
      if (peek() != '"') throw new IllegalStateException("expected string");
      i++;
      StringBuilder sb = new StringBuilder();
      while (i < s.length()) {
        char c = s.charAt(i++);
        if (c == '"') return sb.toString();
        if (c == '\\') {
          char e = s.charAt(i++);
          switch (e) {
            case '"' -> sb.append('"');
            case '\\' -> sb.append('\\');
            case '/' -> sb.append('/');
            case 'n' -> sb.append('\n');
            case 'r' -> sb.append('\r');
            case 't' -> sb.append('\t');
            case 'b' -> sb.append('\b');
            case 'f' -> sb.append('\f');
            case 'u' -> {
              int code = Integer.parseInt(s.substring(i, i + 4), 16);
              i += 4;
              sb.append((char) code);
            }
            default -> throw new IllegalStateException("unknown escape \\" + e);
          }
        } else {
          sb.append(c);
        }
      }
      throw new IllegalStateException("unterminated string");
    }

    private Boolean parseBool() {
      if (s.startsWith("true", i)) {
        i += 4;
        return Boolean.TRUE;
      }
      if (s.startsWith("false", i)) {
        i += 5;
        return Boolean.FALSE;
      }
      throw new IllegalStateException("invalid literal");
    }

    private Object parseNull() {
      if (s.startsWith("null", i)) {
        i += 4;
        return null;
      }
      throw new IllegalStateException("invalid literal");
    }

    private Number parseNumber() {
      int start = i;
      boolean isFloat = false;
      while (i < s.length()) {
        char c = s.charAt(i);
        if ((c >= '0' && c <= '9') || c == '-' || c == '+') {
          i++;
        } else if (c == '.' || c == 'e' || c == 'E') {
          isFloat = true;
          i++;
        } else {
          break;
        }
      }
      String text = s.substring(start, i);
      if (text.isEmpty()) throw new IllegalStateException("invalid number");
      return isFloat ? (Number) Double.parseDouble(text) : (Number) Long.parseLong(text);
    }
  }
}
