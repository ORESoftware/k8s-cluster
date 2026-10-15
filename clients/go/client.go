package networkmutex

import (
	"bufio"
	"context"
	"crypto/rand"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"net"
	"sync"
	"time"
)

// Client is a multiplexed TCP client. One [*Client] is safe for concurrent
// use; many goroutines can call [Client.Acquire] / [Client.Release] / RW
// helpers simultaneously, all sharing the same connection.
type Client struct {
	conn net.Conn
	w    *bufio.Writer
	mu   sync.Mutex // serialises writes to w

	infMu     sync.Mutex
	inflight  map[string]chan Response
	multi     map[string]struct{} // uuids that may receive >1 response
	closeOnce sync.Once
	closed    chan struct{}
	readErr   error
}

// Options configures [Dial].
type Options struct {
	Network        string // "tcp" (default) or "unix"
	Address        string // host:port for tcp, /path/sock for unix
	Token          string // optional shared secret; sent as `auth` on connect
	ConnectTimeout time.Duration
}

func newUUID() string {
	var b [16]byte
	_, _ = rand.Read(b[:])
	b[6] = (b[6] & 0x0f) | 0x40 // v4
	b[8] = (b[8] & 0x3f) | 0x80 // variant
	hexBuf := make([]byte, 36)
	hex.Encode(hexBuf[0:8], b[0:4])
	hexBuf[8] = '-'
	hex.Encode(hexBuf[9:13], b[4:6])
	hexBuf[13] = '-'
	hex.Encode(hexBuf[14:18], b[6:8])
	hexBuf[18] = '-'
	hex.Encode(hexBuf[19:23], b[8:10])
	hexBuf[23] = '-'
	hex.Encode(hexBuf[24:36], b[10:16])
	return string(hexBuf)
}

// Dial connects and (if [Options.Token] is set) performs the auth handshake.
func Dial(ctx context.Context, opts Options) (*Client, error) {
	if opts.Network == "" {
		opts.Network = "tcp"
	}
	if opts.ConnectTimeout == 0 {
		opts.ConnectTimeout = 5 * time.Second
	}
	d := net.Dialer{Timeout: opts.ConnectTimeout}
	if tcpConn, ok := (&net.TCPConn{}).RemoteAddr().(*net.TCPAddr); ok && tcpConn != nil {
		_ = tcpConn // keep the import meaningful for go vet
	}
	conn, err := d.DialContext(ctx, opts.Network, opts.Address)
	if err != nil {
		return nil, fmt.Errorf("dial %s %s: %w", opts.Network, opts.Address, err)
	}
	if tcp, ok := conn.(*net.TCPConn); ok {
		_ = tcp.SetNoDelay(true)
	}

	c := &Client{
		conn:     conn,
		w:        bufio.NewWriter(conn),
		inflight: make(map[string]chan Response),
		multi:    make(map[string]struct{}),
		closed:   make(chan struct{}),
	}
	go c.readLoop()

	if opts.Token != "" {
		uuid := newUUID()
		resp, err := c.roundtrip(ctx, Request{Type: ReqAuth, UUID: uuid, Token: opts.Token})
		if err != nil {
			_ = c.Close()
			return nil, fmt.Errorf("auth: %w", err)
		}
		if resp.Type != RespAuth || resp.OK == nil || !*resp.OK {
			_ = c.Close()
			return nil, fmt.Errorf("auth rejected: %v", resp.Error)
		}
	}
	return c, nil
}

// Close terminates the connection and fails every inflight request.
func (c *Client) Close() error {
	c.closeOnce.Do(func() {
		close(c.closed)
		_ = c.conn.Close()
		c.infMu.Lock()
		for _, ch := range c.inflight {
			close(ch)
		}
		c.inflight = nil
		c.multi = nil
		c.infMu.Unlock()
	})
	return nil
}

// SingleLockHandle is the receipt for an exclusive single-key lock.
type SingleLockHandle struct {
	Key          string
	LockUUID     string
	FencingToken uint64
}

// CompositeLockHandle is the receipt for an atomically-acquired multi-key lock.
type CompositeLockHandle struct {
	Keys          []string
	LockUUID      string
	FencingTokens map[string]uint64
}

// AcquireOptions configures an exclusive single-key acquire. Wait defaults to
// broker-default blocking semantics when nil; pass WaitOption(false) for a
// fail-fast request, or use [Client.TryAcquire].
type AcquireOptions struct {
	TTL        time.Duration
	MaxHolders *int
	Wait       *bool
}

// AcquireManyOptions configures an exclusive composite acquire. Wait defaults
// to broker-default blocking semantics when nil; pass WaitOption(false) for a
// fail-fast request, or use [Client.TryAcquireMany].
type AcquireManyOptions struct {
	TTL  time.Duration
	Wait *bool
}

// WaitOption returns a pointer suitable for AcquireOptions.Wait while preserving
// the distinction between omitted and explicit false.
func WaitOption(wait bool) *bool {
	return &wait
}

// Acquire takes an exclusive lock on a single key.
func (c *Client) Acquire(ctx context.Context, key string, ttl time.Duration) (SingleLockHandle, error) {
	return c.AcquireWithOptions(ctx, key, AcquireOptions{TTL: ttl, Wait: WaitOption(true)})
}

// AcquireWithOptions takes an exclusive lock on a single key. With Wait nil or
// true it blocks until the broker grants the lock; with Wait false it sends a
// no-wait request and returns an error if the key is currently contended.
func (c *Client) AcquireWithOptions(ctx context.Context, key string, opts AcquireOptions) (SingleLockHandle, error) {
	if opts.Wait != nil && !*opts.Wait {
		h, ok, err := c.tryAcquireWithOptions(ctx, key, opts)
		if err != nil {
			return SingleLockHandle{}, err
		}
		if !ok {
			return SingleLockHandle{}, fmt.Errorf("lock(%s) not acquired", key)
		}
		return h, nil
	}
	uuid := newUUID()
	c.markMulti(uuid)
	defer c.unmarkMulti(uuid)
	resp, err := c.roundtripGrant(ctx, Request{
		Type: ReqLock, UUID: uuid, Key: key, TTL: int(opts.TTL.Milliseconds()),
		Max: opts.MaxHolders, Wait: opts.Wait,
	})
	if err != nil {
		return SingleLockHandle{}, err
	}
	if resp.Type != RespLock || resp.Acquired == nil || !*resp.Acquired || resp.LockUUID == "" {
		return SingleLockHandle{}, fmt.Errorf("lock(%s) failed: %+v", key, resp)
	}
	var token uint64
	if resp.FencingToken != nil {
		token = *resp.FencingToken
	}
	return SingleLockHandle{Key: key, LockUUID: resp.LockUUID, FencingToken: token}, nil
}

// TryAcquire attempts a single-key acquire without queuing. It returns ok=false
// immediately when the key is currently contended.
func (c *Client) TryAcquire(ctx context.Context, key string, ttl time.Duration) (SingleLockHandle, bool, error) {
	return c.tryAcquireWithOptions(ctx, key, AcquireOptions{TTL: ttl, Wait: WaitOption(false)})
}

func (c *Client) tryAcquireWithOptions(ctx context.Context, key string, opts AcquireOptions) (SingleLockHandle, bool, error) {
	uuid := newUUID()
	resp, err := c.roundtrip(ctx, Request{
		Type: ReqLock, UUID: uuid, Key: key, TTL: int(opts.TTL.Milliseconds()),
		Max: opts.MaxHolders, Wait: WaitOption(false),
	})
	if err != nil {
		return SingleLockHandle{}, false, err
	}
	if resp.Type == RespError {
		return SingleLockHandle{}, false, fmt.Errorf("try lock(%s): %s", key, resp.Error)
	}
	if resp.Type != RespLock || resp.Acquired == nil {
		return SingleLockHandle{}, false, fmt.Errorf("try lock(%s) unexpected: %+v", key, resp)
	}
	if !*resp.Acquired || resp.LockUUID == "" {
		return SingleLockHandle{}, false, nil
	}
	var token uint64
	if resp.FencingToken != nil {
		token = *resp.FencingToken
	}
	return SingleLockHandle{Key: key, LockUUID: resp.LockUUID, FencingToken: token}, true, nil
}

// AcquireMany atomically acquires up to 5 keys. The broker sorts the keys
// to prevent deadlocks; the caller doesn't have to.
func (c *Client) AcquireMany(ctx context.Context, keys []string, ttl time.Duration) (CompositeLockHandle, error) {
	return c.AcquireManyWithOptions(ctx, keys, AcquireManyOptions{TTL: ttl, Wait: WaitOption(true)})
}

// AcquireManyWithOptions atomically acquires up to 5 keys. With Wait nil or
// true it blocks until every key is granted; with Wait false it sends a no-wait
// request and returns an error if any member key is currently contended.
func (c *Client) AcquireManyWithOptions(ctx context.Context, keys []string, opts AcquireManyOptions) (CompositeLockHandle, error) {
	if n := len(keys); n == 0 || n > 5 {
		return CompositeLockHandle{}, fmt.Errorf("composite key count must be 1..=5, got %d", n)
	}
	if opts.Wait != nil && !*opts.Wait {
		h, ok, err := c.tryAcquireManyWithOptions(ctx, keys, opts)
		if err != nil {
			return CompositeLockHandle{}, err
		}
		if !ok {
			return CompositeLockHandle{}, fmt.Errorf("acquireMany(%v) not acquired", keys)
		}
		return h, nil
	}
	uuid := newUUID()
	c.markMulti(uuid)
	defer c.unmarkMulti(uuid)
	resp, err := c.roundtripGrant(ctx, Request{
		Type: ReqLock, UUID: uuid, Keys: keys, TTL: int(opts.TTL.Milliseconds()), Wait: opts.Wait,
	})
	if err != nil {
		return CompositeLockHandle{}, err
	}
	if resp.Type != RespCompositeLock || resp.Acquired == nil || !*resp.Acquired || resp.LockUUID == "" {
		return CompositeLockHandle{}, fmt.Errorf("acquireMany(%v) failed: %+v", keys, resp)
	}
	return CompositeLockHandle{Keys: keys, LockUUID: resp.LockUUID, FencingTokens: resp.FencingTokens}, nil
}

// TryAcquireMany attempts a composite acquire without queuing. It returns
// ok=false immediately when any member key is currently contended.
func (c *Client) TryAcquireMany(ctx context.Context, keys []string, ttl time.Duration) (CompositeLockHandle, bool, error) {
	return c.tryAcquireManyWithOptions(ctx, keys, AcquireManyOptions{TTL: ttl, Wait: WaitOption(false)})
}

func (c *Client) tryAcquireManyWithOptions(ctx context.Context, keys []string, opts AcquireManyOptions) (CompositeLockHandle, bool, error) {
	if n := len(keys); n == 0 || n > 5 {
		return CompositeLockHandle{}, false, fmt.Errorf("composite key count must be 1..=5, got %d", n)
	}
	uuid := newUUID()
	resp, err := c.roundtrip(ctx, Request{
		Type: ReqLock, UUID: uuid, Keys: keys, TTL: int(opts.TTL.Milliseconds()), Wait: WaitOption(false),
	})
	if err != nil {
		return CompositeLockHandle{}, false, err
	}
	if resp.Type == RespError {
		return CompositeLockHandle{}, false, fmt.Errorf("try acquireMany(%v): %s", keys, resp.Error)
	}
	if resp.Type != RespCompositeLock || resp.Acquired == nil {
		return CompositeLockHandle{}, false, fmt.Errorf("try acquireMany(%v) unexpected: %+v", keys, resp)
	}
	if !*resp.Acquired || resp.LockUUID == "" {
		return CompositeLockHandle{}, false, nil
	}
	return CompositeLockHandle{Keys: keys, LockUUID: resp.LockUUID, FencingTokens: resp.FencingTokens}, true, nil
}

// Release releases either a single or composite lock.
func (c *Client) Release(ctx context.Context, handle any) error {
	var req Request
	switch h := handle.(type) {
	case SingleLockHandle:
		req = Request{Type: ReqUnlock, UUID: newUUID(), Key: h.Key, LockUUID: h.LockUUID}
	case CompositeLockHandle:
		req = Request{Type: ReqUnlock, UUID: newUUID(), Keys: h.Keys, LockUUID: h.LockUUID}
	default:
		return fmt.Errorf("Release: unknown handle type %T", handle)
	}
	resp, err := c.roundtrip(ctx, req)
	if err != nil {
		return err
	}
	if resp.Type != RespUnlock || resp.Unlocked == nil || !*resp.Unlocked {
		return fmt.Errorf("unlock failed: %+v", resp)
	}
	return nil
}

// AcquireRead enqueues as a reader.
func (c *Client) AcquireRead(ctx context.Context, key string) (string, uint64, error) {
	uuid := newUUID()
	c.markMulti(uuid)
	defer c.unmarkMulti(uuid)
	resp, err := c.roundtripUntilGranted(ctx, Request{Type: ReqRegisterRead, UUID: uuid, Key: key})
	if err != nil {
		return "", 0, err
	}
	var t uint64
	if resp.FencingToken != nil {
		t = *resp.FencingToken
	}
	return resp.LockUUID, t, nil
}

// ReleaseRead drops a reader hold.
func (c *Client) ReleaseRead(ctx context.Context, key string) error {
	_, err := c.roundtrip(ctx, Request{Type: ReqEndRead, UUID: newUUID(), Key: key})
	return err
}

// AcquireWrite enqueues as a writer.
func (c *Client) AcquireWrite(ctx context.Context, key string) (string, uint64, error) {
	uuid := newUUID()
	c.markMulti(uuid)
	defer c.unmarkMulti(uuid)
	resp, err := c.roundtripUntilGranted(ctx, Request{Type: ReqRegisterWrite, UUID: uuid, Key: key})
	if err != nil {
		return "", 0, err
	}
	var t uint64
	if resp.FencingToken != nil {
		t = *resp.FencingToken
	}
	return resp.LockUUID, t, nil
}

// ReleaseWrite drops the writer hold.
func (c *Client) ReleaseWrite(ctx context.Context, key string) error {
	_, err := c.roundtrip(ctx, Request{Type: ReqEndWrite, UUID: newUUID(), Key: key})
	return err
}

// -- internals ----------------------------------------------------------

func (c *Client) markMulti(uuid string) {
	c.infMu.Lock()
	c.multi[uuid] = struct{}{}
	c.infMu.Unlock()
}

func (c *Client) unmarkMulti(uuid string) {
	c.infMu.Lock()
	delete(c.multi, uuid)
	c.infMu.Unlock()
}

func (c *Client) roundtrip(ctx context.Context, req Request) (Response, error) {
	ch := make(chan Response, 4)
	c.infMu.Lock()
	if c.inflight == nil {
		c.infMu.Unlock()
		return Response{}, errors.New("client closed")
	}
	c.inflight[req.UUID] = ch
	c.infMu.Unlock()
	defer func() {
		c.infMu.Lock()
		if c.inflight != nil {
			delete(c.inflight, req.UUID)
		}
		c.infMu.Unlock()
	}()

	if err := c.sendFrame(req); err != nil {
		return Response{}, err
	}

	select {
	case <-ctx.Done():
		return Response{}, ctx.Err()
	case <-c.closed:
		if c.readErr != nil {
			return Response{}, c.readErr
		}
		return Response{}, errors.New("client closed")
	case resp, ok := <-ch:
		if !ok {
			return Response{}, errors.New("response channel closed")
		}
		return resp, nil
	}
}

// roundtripGrant accepts the first response that is either a final grant
// (acquired=true, error set, or non-lock variant) — used for `lock` which
// can produce a queued frame followed by the actual grant.
func (c *Client) roundtripGrant(ctx context.Context, req Request) (Response, error) {
	ch := make(chan Response, 4)
	c.infMu.Lock()
	c.inflight[req.UUID] = ch
	c.infMu.Unlock()
	defer func() {
		c.infMu.Lock()
		delete(c.inflight, req.UUID)
		c.infMu.Unlock()
	}()

	if err := c.sendFrame(req); err != nil {
		return Response{}, err
	}

	for {
		select {
		case <-ctx.Done():
			return Response{}, ctx.Err()
		case <-c.closed:
			return Response{}, errors.New("client closed")
		case resp, ok := <-ch:
			if !ok {
				return Response{}, errors.New("response channel closed")
			}
			if resp.Type == RespError {
				return resp, nil
			}
			if resp.Type == RespLock || resp.Type == RespCompositeLock {
				if resp.Acquired != nil && *resp.Acquired {
					return resp, nil
				}
				if resp.Error != "" {
					return resp, nil
				}
				continue
			}
			return resp, nil
		}
	}
}

func (c *Client) roundtripUntilGranted(ctx context.Context, req Request) (Response, error) {
	ch := make(chan Response, 4)
	c.infMu.Lock()
	c.inflight[req.UUID] = ch
	c.infMu.Unlock()
	defer func() {
		c.infMu.Lock()
		delete(c.inflight, req.UUID)
		c.infMu.Unlock()
	}()

	if err := c.sendFrame(req); err != nil {
		return Response{}, err
	}

	for {
		select {
		case <-ctx.Done():
			return Response{}, ctx.Err()
		case <-c.closed:
			return Response{}, errors.New("client closed")
		case resp, ok := <-ch:
			if !ok {
				return Response{}, errors.New("response channel closed")
			}
			if resp.Granted != nil && *resp.Granted {
				return resp, nil
			}
			if resp.Type == RespError {
				return resp, nil
			}
		}
	}
}

func (c *Client) sendFrame(req Request) error {
	frame, err := req.Encode()
	if err != nil {
		return err
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if _, err := c.w.Write(frame); err != nil {
		return err
	}
	return c.w.Flush()
}

func (c *Client) readLoop() {
	defer c.Close()
	scanner := bufio.NewScanner(c.conn)
	scanner.Buffer(make([]byte, 64*1024), 16*1024*1024)
	for scanner.Scan() {
		line := scanner.Bytes()
		if len(line) == 0 {
			continue
		}
		resp, err := Decode(line)
		if err != nil {
			c.readErr = err
			return
		}
		c.dispatch(resp)
	}
	if err := scanner.Err(); err != nil && !errors.Is(err, io.EOF) {
		c.readErr = err
	}
}

func (c *Client) dispatch(resp Response) {
	c.infMu.Lock()
	ch, ok := c.inflight[resp.UUID]
	c.infMu.Unlock()
	if !ok {
		return
	}
	select {
	case ch <- resp:
	default:
		// channel buffer full — drop, the caller has already moved on
	}
}
