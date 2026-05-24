package networkmutex

import (
	"strings"
	"testing"
)

func TestRequestEncodeUsesCamelCaseTagAndFields(t *testing.T) {
	req := Request{
		Type: ReqLock, UUID: "u",
		Keys: []string{"a", "b"},
		TTL:  1000,
	}
	frame, err := req.Encode()
	if err != nil {
		t.Fatal(err)
	}
	s := string(frame)
	for _, needle := range []string{
		`"type":"lock"`, `"uuid":"u"`, `"keys":["a","b"]`, `"ttl":1000`,
	} {
		if !strings.Contains(s, needle) {
			t.Fatalf("missing %q in %s", needle, s)
		}
	}
}

func TestDecodeCompositeLockResponse(t *testing.T) {
	resp, err := Decode([]byte(`{"type":"compositeLock","uuid":"u","keys":["a","b"],"acquired":true,"lockUuid":"L","fencingTokens":{"a":1,"b":2}}`))
	if err != nil {
		t.Fatal(err)
	}
	if resp.Type != RespCompositeLock {
		t.Fatalf("wrong type: %s", resp.Type)
	}
	if resp.LockUUID != "L" || resp.FencingTokens["b"] != 2 {
		t.Fatalf("bad parse: %+v", resp)
	}
}
