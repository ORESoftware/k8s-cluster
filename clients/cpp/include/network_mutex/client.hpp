// Multiplexed TCP client for dd-rust-network-mutex (header-only).
//
// One Client is safe to share across threads: writes are serialized and a
// background reader thread fans broker frames out to per-request queues keyed
// by the correlation uuid (mirrors the Go client's map[string]chan Response).
#pragma once

#include <netdb.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <sys/socket.h>
#include <unistd.h>

#include <atomic>
#include <condition_variable>
#include <cstring>
#include <deque>
#include <map>
#include <memory>
#include <mutex>
#include <random>
#include <stdexcept>
#include <string>
#include <thread>
#include <vector>

#include "protocol.hpp"

namespace nm {

class NetworkMutexError : public std::runtime_error {
 public:
  explicit NetworkMutexError(const std::string& m) : std::runtime_error(m) {}
};

struct SingleLockHandle {
  std::string key;
  std::string lock_uuid;
  uint64_t fencing_token = 0;
};

struct CompositeLockHandle {
  std::vector<std::string> keys;
  std::string lock_uuid;
  std::map<std::string, uint64_t> fencing_tokens;
};

inline std::string new_uuid() {
  static thread_local std::mt19937_64 rng{std::random_device{}()};
  std::uniform_int_distribution<uint64_t> dist;
  uint64_t hi = dist(rng), lo = dist(rng);
  unsigned char b[16];
  for (int i = 0; i < 8; ++i) b[i] = (hi >> (8 * i)) & 0xFF;
  for (int i = 0; i < 8; ++i) b[8 + i] = (lo >> (8 * i)) & 0xFF;
  b[6] = (b[6] & 0x0F) | 0x40;  // version 4
  b[8] = (b[8] & 0x3F) | 0x80;  // variant
  static const char* hex = "0123456789abcdef";
  std::string out;
  out.reserve(36);
  for (int i = 0; i < 16; ++i) {
    if (i == 4 || i == 6 || i == 8 || i == 10) out.push_back('-');
    out.push_back(hex[b[i] >> 4]);
    out.push_back(hex[b[i] & 0x0F]);
  }
  return out;
}

class Client {
 public:
  static std::shared_ptr<Client> connect(const std::string& host = "127.0.0.1",
                                          int port = 6970,
                                          const std::string& token = "") {
    addrinfo hints{};
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    addrinfo* res = nullptr;
    std::string port_s = std::to_string(port);
    if (getaddrinfo(host.c_str(), port_s.c_str(), &hints, &res) != 0 || !res)
      throw NetworkMutexError("getaddrinfo failed for " + host + ":" + port_s);

    int fd = -1;
    for (addrinfo* p = res; p; p = p->ai_next) {
      fd = ::socket(p->ai_family, p->ai_socktype, p->ai_protocol);
      if (fd < 0) continue;
      if (::connect(fd, p->ai_addr, p->ai_addrlen) == 0) break;
      ::close(fd);
      fd = -1;
    }
    freeaddrinfo(res);
    if (fd < 0) throw NetworkMutexError("connect failed for " + host + ":" + port_s);

    int one = 1;
    ::setsockopt(fd, IPPROTO_TCP, TCP_NODELAY, &one, sizeof(one));

    auto c = std::shared_ptr<Client>(new Client(fd));
    c->reader_ = std::thread([c] { c->read_loop(); });
    if (!token.empty()) {
      std::string uuid = new_uuid();
      Response r = c->roundtrip(auth_request(uuid, token), uuid);
      if (r.type != ResponseType::Auth || !r.raw.bool_or("ok")) {
        c->close();
        throw NetworkMutexError("auth rejected: " + r.error);
      }
    }
    return c;
  }

  ~Client() { close(); }

  SingleLockHandle acquire(const std::string& key, uint64_t ttl_ms = 0,
                           std::optional<uint32_t> max_holders = std::nullopt) {
    std::string uuid = new_uuid();
    Response r = roundtrip_grant(lock_request_single(uuid, key, ttl_ms, max_holders, true), uuid);
    if (r.type != ResponseType::Lock || !r.acquired || r.lock_uuid.empty())
      throw NetworkMutexError("lock(" + key + ") failed: " + r.raw.dump());
    return {key, r.lock_uuid, r.fencing_token};
  }

  std::optional<SingleLockHandle> try_acquire(
      const std::string& key, uint64_t ttl_ms = 0,
      std::optional<uint32_t> max_holders = std::nullopt) {
    std::string uuid = new_uuid();
    Response r = roundtrip(lock_request_single(uuid, key, ttl_ms, max_holders, false), uuid);
    if (r.type == ResponseType::Error)
      throw NetworkMutexError("try_acquire(" + key + ") error: " + r.error);
    if (r.type != ResponseType::Lock)
      throw NetworkMutexError("try_acquire(" + key + ") unexpected: " + r.raw.dump());
    if (!r.acquired || r.lock_uuid.empty()) return std::nullopt;
    return SingleLockHandle{key, r.lock_uuid, r.fencing_token};
  }

  CompositeLockHandle acquire_many(const std::vector<std::string>& keys, uint64_t ttl_ms = 0) {
    std::string uuid = new_uuid();
    Response r = roundtrip_grant(lock_request_composite(uuid, keys, ttl_ms, true), uuid);
    if (r.type != ResponseType::CompositeLock || !r.acquired || r.lock_uuid.empty())
      throw NetworkMutexError("acquire_many failed: " + r.raw.dump());
    return {keys, r.lock_uuid, r.fencing_tokens};
  }

  std::optional<CompositeLockHandle> try_acquire_many(
      const std::vector<std::string>& keys, uint64_t ttl_ms = 0) {
    std::string uuid = new_uuid();
    Response r = roundtrip(lock_request_composite(uuid, keys, ttl_ms, false), uuid);
    if (r.type == ResponseType::Error)
      throw NetworkMutexError("try_acquire_many error: " + r.error);
    if (r.type != ResponseType::CompositeLock)
      throw NetworkMutexError("try_acquire_many unexpected: " + r.raw.dump());
    if (!r.acquired || r.lock_uuid.empty()) return std::nullopt;
    return CompositeLockHandle{keys, r.lock_uuid, r.fencing_tokens};
  }

  void release(const SingleLockHandle& h) {
    std::string uuid = new_uuid();
    Response r = roundtrip(unlock_request_single(uuid, h.key, h.lock_uuid), uuid);
    if (r.type != ResponseType::Unlock || !r.unlocked)
      throw NetworkMutexError("unlock failed: " + r.raw.dump());
  }

  void release(const CompositeLockHandle& h) {
    std::string uuid = new_uuid();
    Response r = roundtrip(unlock_request_composite(uuid, h.keys, h.lock_uuid), uuid);
    if (r.type != ResponseType::Unlock || !r.unlocked)
      throw NetworkMutexError("unlock composite failed: " + r.raw.dump());
  }

  SingleLockHandle acquire_read(const std::string& key) {
    std::string uuid = new_uuid();
    Response r = roundtrip_until_granted(rw_request(RequestType::RegisterRead, uuid, key), uuid);
    return {key, r.lock_uuid, r.fencing_token};
  }

  SingleLockHandle acquire_write(const std::string& key) {
    std::string uuid = new_uuid();
    Response r = roundtrip_until_granted(rw_request(RequestType::RegisterWrite, uuid, key), uuid);
    return {key, r.lock_uuid, r.fencing_token};
  }

  void release_read(const std::string& key) {
    std::string uuid = new_uuid();
    roundtrip(rw_request(RequestType::EndRead, uuid, key), uuid);
  }

  void release_write(const std::string& key) {
    std::string uuid = new_uuid();
    roundtrip(rw_request(RequestType::EndWrite, uuid, key), uuid);
  }

  std::vector<std::string> ls() {
    std::string uuid = new_uuid();
    Response r = roundtrip(ls_request(uuid), uuid);
    return r.keys;
  }

  Response lock_info(const std::string& key) {
    std::string uuid = new_uuid();
    return roundtrip(nm::lock_info_request(uuid, key), uuid);
  }

  void close() {
    bool expected = false;
    if (!closed_.compare_exchange_strong(expected, true)) return;
    if (fd_ >= 0) {
      ::shutdown(fd_, SHUT_RDWR);
      ::close(fd_);
    }
    {
      std::lock_guard<std::mutex> lk(mu_);
      for (auto& [uuid, slot] : inflight_) slot->done = true;
      cv_.notify_all();
    }
    if (reader_.joinable() && std::this_thread::get_id() != reader_.get_id()) reader_.join();
  }

 private:
  struct Slot {
    std::deque<Response> q;
    bool done = false;
  };

  explicit Client(int fd) : fd_(fd) {}

  int fd_;
  std::thread reader_;
  std::mutex wmu_;
  std::mutex mu_;
  std::condition_variable cv_;
  std::map<std::string, std::shared_ptr<Slot>> inflight_;
  std::atomic<bool> closed_{false};
  std::string read_err_;

  std::shared_ptr<Slot> register_slot(const std::string& uuid) {
    auto slot = std::make_shared<Slot>();
    std::lock_guard<std::mutex> lk(mu_);
    if (closed_) throw NetworkMutexError("client closed");
    inflight_[uuid] = slot;
    return slot;
  }

  void unregister(const std::string& uuid) {
    std::lock_guard<std::mutex> lk(mu_);
    inflight_.erase(uuid);
  }

  void send(const std::string& frame) {
    std::lock_guard<std::mutex> lk(wmu_);
    size_t off = 0;
    while (off < frame.size()) {
      ssize_t n = ::send(fd_, frame.data() + off, frame.size() - off, 0);
      if (n <= 0) throw NetworkMutexError("send failed");
      off += static_cast<size_t>(n);
    }
  }

  Response next(const std::shared_ptr<Slot>& slot) {
    std::unique_lock<std::mutex> lk(mu_);
    cv_.wait(lk, [&] { return !slot->q.empty() || slot->done; });
    if (!slot->q.empty()) {
      Response r = slot->q.front();
      slot->q.pop_front();
      return r;
    }
    throw NetworkMutexError(read_err_.empty() ? "client closed" : read_err_);
  }

  Response roundtrip(const std::string& frame, const std::string& uuid) {
    auto slot = register_slot(uuid);
    struct Guard {
      Client* c;
      std::string u;
      ~Guard() { c->unregister(u); }
    } guard{this, uuid};
    send(frame);
    return next(slot);
  }

  Response roundtrip_grant(const std::string& frame, const std::string& uuid) {
    auto slot = register_slot(uuid);
    struct Guard {
      Client* c;
      std::string u;
      ~Guard() { c->unregister(u); }
    } guard{this, uuid};
    send(frame);
    for (;;) {
      Response r = next(slot);
      if (r.type == ResponseType::Error) return r;
      if (r.type == ResponseType::Lock || r.type == ResponseType::CompositeLock) {
        if (r.acquired || !r.error.empty()) return r;
        continue;  // queued notice
      }
      return r;
    }
  }

  Response roundtrip_until_granted(const std::string& frame, const std::string& uuid) {
    auto slot = register_slot(uuid);
    struct Guard {
      Client* c;
      std::string u;
      ~Guard() { c->unregister(u); }
    } guard{this, uuid};
    send(frame);
    for (;;) {
      Response r = next(slot);
      if (r.granted) return r;
      if (r.type == ResponseType::Error) throw NetworkMutexError("rw acquire failed: " + r.error);
    }
  }

  void read_loop() {
    std::string buf;
    char chunk[65536];
    while (!closed_) {
      ssize_t n = ::recv(fd_, chunk, sizeof(chunk), 0);
      if (n <= 0) break;
      buf.append(chunk, static_cast<size_t>(n));
      size_t pos;
      while ((pos = buf.find('\n')) != std::string::npos) {
        std::string line = buf.substr(0, pos);
        buf.erase(0, pos + 1);
        if (line.empty()) continue;
        try {
          dispatch(Response::parse(line));
        } catch (const std::exception& e) {
          read_err_ = e.what();
        }
      }
    }
    close();
  }

  void dispatch(Response r) {
    std::lock_guard<std::mutex> lk(mu_);
    auto it = inflight_.find(r.uuid);
    if (it == inflight_.end()) return;
    it->second->q.push_back(std::move(r));
    cv_.notify_all();
  }
};

}  // namespace nm
