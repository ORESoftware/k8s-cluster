package networkmutex

import (
	"bufio"
	"context"
	"encoding/json"
	"net"
	"testing"
	"time"
)

func newPipeClient(t *testing.T) (*Client, net.Conn) {
	t.Helper()
	clientConn, serverConn := net.Pipe()
	c := &Client{
		conn:     clientConn,
		w:        bufio.NewWriter(clientConn),
		inflight: make(map[string]chan Response),
		multi:    make(map[string]struct{}),
		closed:   make(chan struct{}),
	}
	go c.readLoop()
	t.Cleanup(func() {
		_ = c.Close()
		_ = serverConn.Close()
	})
	return c, serverConn
}

func readRequest(t *testing.T, r *bufio.Reader) Request {
	t.Helper()
	line, err := r.ReadBytes('\n')
	if err != nil {
		t.Fatalf("read request: %v", err)
	}
	var req Request
	if err := json.Unmarshal(line, &req); err != nil {
		t.Fatalf("decode request %s: %v", string(line), err)
	}
	return req
}

func writeJSONLine(t *testing.T, conn net.Conn, payload map[string]any) {
	t.Helper()
	frame, err := json.Marshal(payload)
	if err != nil {
		t.Fatal(err)
	}
	frame = append(frame, '\n')
	if _, err := conn.Write(frame); err != nil {
		t.Fatalf("write response: %v", err)
	}
}

func TestTryAcquireManySendsWaitFalseAndReturnsNotAcquired(t *testing.T) {
	client, server := newPipeClient(t)
	reader := bufio.NewReader(server)

	done := make(chan struct{})
	go func() {
		defer close(done)
		req := readRequest(t, reader)
		if req.Type != ReqLock || req.Wait == nil || *req.Wait {
			t.Fatalf("expected composite lock wait=false request, got %+v", req)
		}
		writeJSONLine(t, server, map[string]any{
			"type":     "compositeLock",
			"uuid":     req.UUID,
			"keys":     req.Keys,
			"acquired": false,
		})
	}()

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	_, ok, err := client.TryAcquireMany(ctx, []string{"a", "b"}, time.Second)
	if err != nil {
		t.Fatal(err)
	}
	if ok {
		t.Fatal("expected contended no-wait composite to return ok=false")
	}
	<-done
}

func TestAcquireManySendsWaitTrueAndIgnoresQueuedNotice(t *testing.T) {
	client, server := newPipeClient(t)
	reader := bufio.NewReader(server)

	done := make(chan struct{})
	go func() {
		defer close(done)
		req := readRequest(t, reader)
		if req.Type != ReqLock || req.Wait == nil || !*req.Wait {
			t.Fatalf("expected composite lock wait=true request, got %+v", req)
		}
		writeJSONLine(t, server, map[string]any{
			"type":     "compositeLock",
			"uuid":     req.UUID,
			"keys":     req.Keys,
			"acquired": false,
		})
		writeJSONLine(t, server, map[string]any{
			"type":          "compositeLock",
			"uuid":          req.UUID,
			"keys":          req.Keys,
			"acquired":      true,
			"lockUuid":      "L-1",
			"fencingTokens": map[string]uint64{"a": 1, "b": 2},
		})
	}()

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	handle, err := client.AcquireMany(ctx, []string{"a", "b"}, time.Second)
	if err != nil {
		t.Fatal(err)
	}
	if handle.LockUUID != "L-1" || handle.FencingTokens["b"] != 2 {
		t.Fatalf("bad grant handle: %+v", handle)
	}
	<-done
}
