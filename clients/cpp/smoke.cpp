// End-to-end smoke test mirroring clients/go/cmd/smoke/main.go.
//
//   make smoke && ./build/smoke
//
// Override host/port via LIVE_MUTEX_HOST / LIVE_MUTEX_PORT.
#include <cstdlib>
#include <future>
#include <iostream>
#include <string>
#include <vector>

#include "network_mutex/client.hpp"

static std::string env_or(const char* key, const std::string& def) {
  const char* v = std::getenv(key);
  return (v && *v) ? std::string(v) : def;
}

int main() {
  std::string host = env_or("LIVE_MUTEX_HOST", "127.0.0.1");
  int port = std::stoi(env_or("LIVE_MUTEX_PORT", "6970"));

  try {
    auto client = nm::Client::connect(host, port);
    std::cout << "[smoke-cpp] connected " << host << ":" << port << "\n";

    auto ex = client->acquire("smoke-cpp-exclusive", 5000);
    std::cout << "[smoke-cpp] exclusive grant: lockUuid=" << ex.lock_uuid
              << " fencing=" << ex.fencing_token << "\n";
    client->release(ex);
    std::cout << "[smoke-cpp] released exclusive\n";

    auto comp = client->acquire_many({"smoke-cpp-a", "smoke-cpp-b", "smoke-cpp-c"}, 5000);
    std::cout << "[smoke-cpp] composite grant: lockUuid=" << comp.lock_uuid << " tokens={";
    for (const auto& [k, v] : comp.fencing_tokens) std::cout << k << ":" << v << " ";
    std::cout << "}\n";
    client->release(comp);
    std::cout << "[smoke-cpp] released composite\n";

    auto w = client->acquire_write("smoke-cpp-rw");
    std::cout << "[smoke-cpp] writer grant: id=" << w.lock_uuid << " fencing=" << w.fencing_token
              << "\n";
    client->release_write("smoke-cpp-rw");

    auto r1 = std::async(std::launch::async, [&] { return client->acquire_read("smoke-cpp-rw"); });
    auto r2 = std::async(std::launch::async, [&] { return client->acquire_read("smoke-cpp-rw"); });
    auto h1 = r1.get();
    auto h2 = r2.get();
    std::cout << "[smoke-cpp] readers: " << h1.fencing_token << " " << h2.fencing_token << "\n";
    client->release_read("smoke-cpp-rw");
    client->release_read("smoke-cpp-rw");

    client->close();
    std::cout << "[smoke-cpp] OK\n";
    return 0;
  } catch (const std::exception& e) {
    std::cerr << "[smoke-cpp] FAILED: " << e.what() << "\n";
    return 1;
  }
}
