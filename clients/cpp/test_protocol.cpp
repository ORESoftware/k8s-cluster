// Offline protocol round-trip tests (no broker needed).
//
//   make test && ./build/test_protocol
#include <cassert>
#include <iostream>
#include <optional>
#include <string>
#include <type_traits>
#include <utility>

#include "network_mutex/client.hpp"

static int failures = 0;
#define CHECK(cond)                                                       \
  do {                                                                    \
    if (!(cond)) {                                                        \
      std::cerr << "FAIL: " << #cond << " (" << __FILE__ << ":" << __LINE__ \
                << ")\n";                                                 \
      ++failures;                                                         \
    }                                                                     \
  } while (0)

static_assert(std::is_same_v<
              decltype(std::declval<nm::Client&>().try_acquire(std::declval<const std::string&>())),
              std::optional<nm::SingleLockHandle>>);
static_assert(std::is_same_v<
              decltype(std::declval<nm::Client&>().try_acquire_many(
                  std::declval<const std::vector<std::string>&>())),
              std::optional<nm::CompositeLockHandle>>);

int main() {
  using namespace nm;

  // Request: camelCase wire fields, newline-terminated.
  {
    std::string f = lock_request_single("u-1", "k1", 4000, 1);
    CHECK(f.back() == '\n');
    json::Parser p(f);
    json::Value v = p.parse();
    CHECK(v.str_or("type") == "lock");
    CHECK(v.str_or("key") == "k1");
    CHECK(v.u64_or("ttl") == 4000);
    CHECK(v.u64_or("max") == 1);
    CHECK(!v.contains("keys"));
    CHECK(!v.contains("wait"));
  }

  // Explicit wait=true is preserved for blocking lock requests.
  {
    std::string f = lock_request_single("u-wait", "k1", 4000, std::nullopt, true);
    json::Parser p(f);
    json::Value v = p.parse();
    CHECK(v.bool_or("wait", false) == true);
  }

  // Composite request: keys preserved unsorted (broker sorts).
  {
    std::string f = lock_request_composite("u-2", {"c", "a", "b"}, 0, false);
    json::Parser p(f);
    json::Value v = p.parse();
    CHECK(v.str_or("type") == "lock");
    CHECK(v.bool_or("wait", true) == false);
    const json::Value* ks = v.find("keys");
    CHECK(ks && ks->as_array().size() == 3);
    CHECK(ks->as_array()[0].as_string() == "c");
  }

  // Composite oversize rejected.
  {
    bool threw = false;
    try {
      lock_request_composite("u", {"a", "b", "c", "d", "e", "f"});
    } catch (const std::invalid_argument&) {
      threw = true;
    }
    CHECK(threw);
  }

  // Response: composite grant parse, 64-bit token precision.
  {
    Response r = Response::parse(
        R"({"type":"compositeLock","uuid":"u-1","keys":["a","b"],"acquired":true,)"
        R"("lockUuid":"L-1","fencingTokens":{"a":1780240060223,"b":12}})");
    CHECK(r.type == ResponseType::CompositeLock);
    CHECK(r.acquired);
    CHECK(r.lock_uuid == "L-1");
    CHECK(r.fencing_tokens["a"] == 1780240060223ULL);
    CHECK(r.fencing_tokens["b"] == 12ULL);
  }

  // Response: unknown type degrades to Unknown rather than crashing.
  {
    Response r = Response::parse(R"({"type":"totallyBogus","uuid":"u"})");
    CHECK(r.type == ResponseType::Unknown);
    CHECK(r.uuid == "u");
  }

  // Response: single lock grant.
  {
    Response r = Response::parse(
        R"({"type":"lock","uuid":"u","key":"k","acquired":true,"lockUuid":"L","fencingToken":7,"readersCount":0})");
    CHECK(r.type == ResponseType::Lock);
    CHECK(r.acquired);
    CHECK(r.fencing_token == 7);
  }

  if (failures == 0) {
    std::cout << "[test-cpp] all protocol tests passed\n";
    return 0;
  }
  std::cerr << "[test-cpp] " << failures << " failure(s)\n";
  return 1;
}
