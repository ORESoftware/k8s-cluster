// Package networkmutex is the Go client for the rust-network-mutex-rs broker.
//
// This file is the Go mirror of `src/protocol.rs`. The wire format is
// camelCase, and we keep typed const blocks for the `type` discriminator
// instead of bare strings so the compiler/linter can catch typos and a
// switch over [RequestType]/[ResponseType] is exhaustively reviewable.
package networkmutex

import (
	"encoding/json"
	"fmt"
)

// RequestType is the discriminator for client-to-broker frames.
type RequestType string

const (
	ReqVersion       RequestType = "version"
	ReqAuth          RequestType = "auth"
	ReqLock          RequestType = "lock"
	ReqUnlock        RequestType = "unlock"
	ReqRegisterRead  RequestType = "registerRead"
	ReqRegisterWrite RequestType = "registerWrite"
	ReqEndRead       RequestType = "endRead"
	ReqEndWrite      RequestType = "endWrite"
	ReqLockInfo      RequestType = "lockInfo"
	ReqLs            RequestType = "ls"
	ReqHeartbeat     RequestType = "heartbeat"
)

// ResponseType is the discriminator for broker-to-client frames.
type ResponseType string

const (
	RespVersion             ResponseType = "version"
	RespAuth                ResponseType = "auth"
	RespLock                ResponseType = "lock"
	RespCompositeLock       ResponseType = "compositeLock"
	RespUnlock              ResponseType = "unlock"
	RespRegisterReadResult  ResponseType = "registerReadResult"
	RespRegisterWriteResult ResponseType = "registerWriteResult"
	RespEndReadResult       ResponseType = "endReadResult"
	RespEndWriteResult      ResponseType = "endWriteResult"
	RespLockInfo            ResponseType = "lockInfo"
	RespLsResult            ResponseType = "lsResult"
	RespReelection          ResponseType = "reelection"
	RespError               ResponseType = "error"
	RespOk                  ResponseType = "ok"
)

// Request is a tagged union over [RequestType]. We use a single struct with
// `omitempty` rather than per-variant types because Go does not have sum
// types in the std library; the constructors below give back type-safe
// values.
type Request struct {
	Type RequestType `json:"type"`
	UUID string      `json:"uuid"`

	// version
	Value string `json:"value,omitempty"`

	// auth
	Token string `json:"token,omitempty"`

	// lock / unlock
	Key                 string   `json:"key,omitempty"`
	Keys                []string `json:"keys,omitempty"`
	PID                 int      `json:"pid,omitempty"`
	TTL                 int      `json:"ttl,omitempty"`
	Max                 *int     `json:"max,omitempty"`
	Force               bool     `json:"force,omitempty"`
	RetryCount          int      `json:"retryCount,omitempty"`
	KeepLocksAfterDeath bool     `json:"keepLocksAfterDeath,omitempty"`
	LockUUID            string   `json:"lockUuid,omitempty"`
}

// Response is the parsed broker frame. Most fields are pointers so that
// "absent" and "false / 0" can be distinguished.
type Response struct {
	Type ResponseType `json:"type"`
	UUID string       `json:"uuid"`

	BrokerVersion string `json:"brokerVersion,omitempty"`
	OK            *bool  `json:"ok,omitempty"`
	Error         string `json:"error,omitempty"`

	Key      string   `json:"key,omitempty"`
	Keys     []string `json:"keys,omitempty"`
	Acquired *bool    `json:"acquired,omitempty"`
	Unlocked *bool    `json:"unlocked,omitempty"`

	LockRequestCount *int              `json:"lockRequestCount,omitempty"`
	LockUUID         string            `json:"lockUuid,omitempty"`
	FencingToken     *uint64           `json:"fencingToken,omitempty"`
	FencingTokens    map[string]uint64 `json:"fencingTokens,omitempty"`
	ReadersCount     *int              `json:"readersCount,omitempty"`
	WriterFlag       *bool             `json:"writerFlag,omitempty"`
	Granted          *bool             `json:"granted,omitempty"`
	IsLocked         *bool             `json:"isLocked,omitempty"`
	LockholderUUIDs  []string          `json:"lockholderUuids,omitempty"`
}

// Encode marshals a request to a single newline-delimited JSON frame.
func (r Request) Encode() ([]byte, error) {
	buf, err := json.Marshal(r)
	if err != nil {
		return nil, fmt.Errorf("encode request: %w", err)
	}
	return append(buf, '\n'), nil
}

// Decode parses a single broker frame.
func Decode(buf []byte) (Response, error) {
	var resp Response
	if err := json.Unmarshal(buf, &resp); err != nil {
		return Response{}, fmt.Errorf("decode response: %w", err)
	}
	return resp, nil
}
