// Wire protocol for dd-rust-network-mutex (C++ mirror of src/protocol.rs).
//
// camelCase, newline-delimited JSON. We keep strongly-typed enum classes for
// the `type` discriminator and a switch over every variant, rather than the
// bare-string `if (data.type === "...")` chains of the upstream Node library.
// See ../../PROTOCOL.md for the single source of truth.
#pragma once

#include <cstdint>
#include <map>
#include <optional>
#include <stdexcept>
#include <string>
#include <vector>

#include "json.hpp"

namespace nm {

constexpr int kMaxCompositeKeys = 5;
constexpr const char* kProtocolVersion = "0.1.0";

enum class RequestType {
  Version,
  Auth,
  Lock,
  Unlock,
  RegisterRead,
  RegisterWrite,
  EndRead,
  EndWrite,
  LockInfo,
  Ls,
  Heartbeat,
};

enum class ResponseType {
  Version,
  Auth,
  Lock,
  CompositeLock,
  Unlock,
  RegisterReadResult,
  RegisterWriteResult,
  EndReadResult,
  EndWriteResult,
  LockInfo,
  LsResult,
  Reelection,
  Error,
  Ok,
  Unknown,
};

inline const char* to_wire(RequestType t) {
  switch (t) {
    case RequestType::Version: return "version";
    case RequestType::Auth: return "auth";
    case RequestType::Lock: return "lock";
    case RequestType::Unlock: return "unlock";
    case RequestType::RegisterRead: return "registerRead";
    case RequestType::RegisterWrite: return "registerWrite";
    case RequestType::EndRead: return "endRead";
    case RequestType::EndWrite: return "endWrite";
    case RequestType::LockInfo: return "lockInfo";
    case RequestType::Ls: return "ls";
    case RequestType::Heartbeat: return "heartbeat";
  }
  return "";
}

inline ResponseType response_type_from_wire(const std::string& s) {
  if (s == "version") return ResponseType::Version;
  if (s == "auth") return ResponseType::Auth;
  if (s == "lock") return ResponseType::Lock;
  if (s == "compositeLock") return ResponseType::CompositeLock;
  if (s == "unlock") return ResponseType::Unlock;
  if (s == "registerReadResult") return ResponseType::RegisterReadResult;
  if (s == "registerWriteResult") return ResponseType::RegisterWriteResult;
  if (s == "endReadResult") return ResponseType::EndReadResult;
  if (s == "endWriteResult") return ResponseType::EndWriteResult;
  if (s == "lockInfo") return ResponseType::LockInfo;
  if (s == "lsResult") return ResponseType::LsResult;
  if (s == "reelection") return ResponseType::Reelection;
  if (s == "error") return ResponseType::Error;
  if (s == "ok") return ResponseType::Ok;
  return ResponseType::Unknown;
}

// ---- request builders -> a single newline-terminated JSON frame -----------

inline std::string frame(const json::Object& obj) {
  return json::Value(obj).dump() + "\n";
}

inline std::string version_request(const std::string& uuid,
                                   const std::string& value = kProtocolVersion) {
  return frame({{"type", to_wire(RequestType::Version)}, {"uuid", uuid}, {"value", value}});
}

inline std::string auth_request(const std::string& uuid, const std::string& token) {
  return frame({{"type", to_wire(RequestType::Auth)}, {"uuid", uuid}, {"token", token}});
}

inline std::string lock_request_single(const std::string& uuid, const std::string& key,
                                       uint64_t ttl_ms = 0,
                                       std::optional<uint32_t> max_holders = std::nullopt,
                                       std::optional<bool> wait = std::nullopt) {
  json::Object o{{"type", to_wire(RequestType::Lock)}, {"uuid", uuid}, {"key", key}};
  if (ttl_ms) o["ttl"] = json::Value(static_cast<uint64_t>(ttl_ms));
  if (max_holders) o["max"] = json::Value(static_cast<uint64_t>(*max_holders));
  if (wait) o["wait"] = json::Value(*wait);
  return frame(o);
}

inline std::string lock_request_composite(const std::string& uuid,
                                          const std::vector<std::string>& keys,
                                          uint64_t ttl_ms = 0,
                                          std::optional<bool> wait = std::nullopt) {
  if (keys.empty() || keys.size() > kMaxCompositeKeys)
    throw std::invalid_argument("composite key count must be 1..=5");
  json::Array arr;
  for (const auto& k : keys) arr.emplace_back(k);
  json::Object o{{"type", to_wire(RequestType::Lock)}, {"uuid", uuid}, {"keys", std::move(arr)}};
  if (ttl_ms) o["ttl"] = json::Value(static_cast<uint64_t>(ttl_ms));
  if (wait) o["wait"] = json::Value(*wait);
  return frame(o);
}

inline std::string unlock_request_single(const std::string& uuid, const std::string& key,
                                         const std::string& lock_uuid, bool force = false) {
  json::Object o{{"type", to_wire(RequestType::Unlock)}, {"uuid", uuid}, {"key", key}};
  if (!lock_uuid.empty()) o["lockUuid"] = json::Value(lock_uuid);
  if (force) o["force"] = json::Value(true);
  return frame(o);
}

inline std::string unlock_request_composite(const std::string& uuid,
                                            const std::vector<std::string>& keys,
                                            const std::string& lock_uuid) {
  json::Array arr;
  for (const auto& k : keys) arr.emplace_back(k);
  json::Object o{{"type", to_wire(RequestType::Unlock)}, {"uuid", uuid}, {"keys", std::move(arr)}};
  if (!lock_uuid.empty()) o["lockUuid"] = json::Value(lock_uuid);
  return frame(o);
}

inline std::string rw_request(RequestType t, const std::string& uuid, const std::string& key) {
  return frame({{"type", to_wire(t)}, {"uuid", uuid}, {"key", key}});
}

inline std::string lock_info_request(const std::string& uuid, const std::string& key) {
  return frame({{"type", to_wire(RequestType::LockInfo)}, {"uuid", uuid}, {"key", key}});
}

inline std::string ls_request(const std::string& uuid) {
  return frame({{"type", to_wire(RequestType::Ls)}, {"uuid", uuid}});
}

// ---- parsed response ------------------------------------------------------

struct Response {
  ResponseType type = ResponseType::Unknown;
  std::string uuid;
  json::Value raw;

  std::string key;
  std::vector<std::string> keys;
  bool acquired = false;
  bool unlocked = false;
  bool granted = false;
  bool is_locked = false;
  bool writer_flag = false;
  std::string lock_uuid;
  std::string error;
  uint64_t fencing_token = 0;
  uint32_t readers_count = 0;
  std::map<std::string, uint64_t> fencing_tokens;

  static Response parse(const std::string& line) {
    json::Parser parser(line);
    json::Value v = parser.parse();
    Response r;
    r.raw = v;
    r.type = response_type_from_wire(v.str_or("type"));
    r.uuid = v.str_or("uuid");
    r.key = v.str_or("key");
    r.lock_uuid = v.str_or("lockUuid");
    r.error = v.str_or("error");
    r.acquired = v.bool_or("acquired");
    r.unlocked = v.bool_or("unlocked");
    r.granted = v.bool_or("granted");
    r.is_locked = v.bool_or("isLocked");
    r.writer_flag = v.bool_or("writerFlag");
    r.fencing_token = v.u64_or("fencingToken");
    r.readers_count = static_cast<uint32_t>(v.u64_or("readersCount"));
    if (const json::Value* ks = v.find("keys"); ks && ks->type() == json::Type::Array) {
      for (const auto& e : ks->as_array()) r.keys.push_back(e.as_string());
    }
    if (const json::Value* ft = v.find("fencingTokens"); ft && ft->type() == json::Type::Object) {
      for (const auto& [k, val] : ft->as_object()) r.fencing_tokens[k] = val.as_u64();
    }
    return r;
  }
};

}  // namespace nm
