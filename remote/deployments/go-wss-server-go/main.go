// dd-go-wss-server
//
// Minimal high-throughput Go WebSocket server purpose-built as the Go
// peer in the cross-language WSS benchmark (Dart, Rust, Gleam, Akka,
// Go). Mirrors dd-rust-wss-server's shape so the comparison is fair:
//
//   - Single pod, single Go process. Concurrency comes from goroutines
//     scheduled across GOMAXPROCS OS threads (the host gives us up to
//     6 vCPUs).
//   - The WS port (default 8107) is bound by N independent acceptor
//     goroutines, each owning its own net.Listener with SO_REUSEPORT.
//     The kernel hashes incoming SYNs across the listeners — same
//     model as Dart's gateway-shard isolates and the Rust acceptor
//     tasks.
//   - The admin port (default 8108) hosts /metrics, /healthz, /readyz
//     on a separate net/http server so probe + Prometheus traffic can
//     never queue behind WS work.
//
// Wire protocol (kept identical to dd-rust-wss-server so the same
// ws-loadtest-rs LOAD_MODE=pipeline driver works verbatim):
//
//	Inbound:
//	  {"type":"ping","id":"<id>","ts":<u64>}      → pong-style reply
//	  {"id":"<id>","payload":"..."}               → akka-style ok-result
//	  text "ping"                                  → "{\"type\":\"pong\",\"ts\":<ms>}"
//
// Anything else is dropped silently.

package main

import (
	"context"
	"errors"
	"fmt"
	"log"
	"net"
	"net/http"
	"os"
	"os/signal"
	"runtime"
	"strconv"
	"strings"
	"sync/atomic"
	"syscall"
	"time"

	"github.com/gorilla/websocket"
	telemetry "github.com/oresoftware/dd/libs/telemetry-go"
	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promhttp"
	"golang.org/x/sys/unix"
)

// ---- metrics ---------------------------------------------------------

var (
	startedAt = time.Now()

	wsActive = prometheus.NewGauge(prometheus.GaugeOpts{
		Name: "dd_go_ws_active",
		Help: "Currently connected WebSocket clients.",
	})
	wsConnsTotal = prometheus.NewCounter(prometheus.CounterOpts{
		Name: "dd_go_ws_connections_total",
		Help: "Accepted WebSocket connections.",
	})
	wsClosedTotal = prometheus.NewCounter(prometheus.CounterOpts{
		Name: "dd_go_ws_closed_total",
		Help: "Closed WebSocket connections.",
	})
	wsMsgsInTotal = prometheus.NewCounter(prometheus.CounterOpts{
		Name: "dd_go_ws_messages_in_total",
		Help: "Inbound WS frames (text+binary).",
	})
	wsMsgsOutTotal = prometheus.NewCounter(prometheus.CounterOpts{
		Name: "dd_go_ws_messages_out_total",
		Help: "Outbound WS frames.",
	})
	wsHandshakeFailTotal = prometheus.NewCounter(prometheus.CounterOpts{
		Name: "dd_go_ws_handshake_failures_total",
		Help: "Failed WS upgrades.",
	})
	wsAcceptors = prometheus.NewGaugeVec(prometheus.GaugeOpts{
		Name: "dd_go_ws_acceptors",
		Help: "Acceptor goroutines bound on the WS port (id label).",
	}, []string{"id"})
	uptimeSec = prometheus.NewGaugeFunc(prometheus.GaugeOpts{
		Name: "dd_go_ws_uptime_seconds",
		Help: "Seconds since process start.",
	}, func() float64 {
		return time.Since(startedAt).Seconds()
	})
)

func init() {
	prometheus.MustRegister(
		wsActive, wsConnsTotal, wsClosedTotal,
		wsMsgsInTotal, wsMsgsOutTotal, wsHandshakeFailTotal,
		wsAcceptors, uptimeSec,
	)
}

var ready atomic.Bool

// ---- env helpers -----------------------------------------------------

func envOr(name, def string) string {
	if v := os.Getenv(name); v != "" {
		return v
	}
	return def
}

func envInt(name string, def int) int {
	if v := os.Getenv(name); v != "" {
		if i, err := strconv.Atoi(v); err == nil && i > 0 {
			return i
		}
	}
	return def
}

// ---- SO_REUSEPORT listener -------------------------------------------

// reuseportListenConfig builds a net.ListenConfig whose Control hook
// sets SO_REUSEADDR + SO_REUSEPORT so multiple acceptor goroutines can
// bind the same TCP port and let the kernel hash incoming SYNs across
// them. Equivalent to Dart's `HttpServer.bind(..., shared: true)` and
// the Rust acceptor's `socket.set_reuseport(true)`.
func reuseportListenConfig() net.ListenConfig {
	return net.ListenConfig{
		Control: func(network, address string, c syscall.RawConn) error {
			var setErr error
			ctrlErr := c.Control(func(fd uintptr) {
				if err := unix.SetsockoptInt(int(fd), unix.SOL_SOCKET, unix.SO_REUSEADDR, 1); err != nil {
					setErr = err
					return
				}
				if err := unix.SetsockoptInt(int(fd), unix.SOL_SOCKET, unix.SO_REUSEPORT, 1); err != nil {
					setErr = err
					return
				}
			})
			if ctrlErr != nil {
				return ctrlErr
			}
			return setErr
		},
	}
}

// ---- WS handler ------------------------------------------------------

// extractIDFromJSON pulls the value of the "id" field out of a JSON
// text frame without allocating a full map[string]interface{}. We
// scan once for `"id"` then walk past colons, whitespace, opening
// quote and capture until the matching close-quote (handling backslash
// escapes minimally — payloads from the loader are deterministic).
func extractIDFromJSON(text []byte) (string, bool) {
	const key = `"id"`
	i := 0
	for {
		idx := strings.Index(string(text[i:]), key)
		if idx < 0 {
			return "", false
		}
		j := i + idx + len(key)
		// skip whitespace + colon + whitespace + opening quote
		for j < len(text) && (text[j] == ' ' || text[j] == '\t') {
			j++
		}
		if j >= len(text) || text[j] != ':' {
			i = j
			continue
		}
		j++
		for j < len(text) && (text[j] == ' ' || text[j] == '\t') {
			j++
		}
		if j >= len(text) || text[j] != '"' {
			i = j
			continue
		}
		j++
		start := j
		for j < len(text) && text[j] != '"' {
			if text[j] == '\\' && j+1 < len(text) {
				j += 2
				continue
			}
			j++
		}
		if j >= len(text) {
			return "", false
		}
		return string(text[start:j]), true
	}
}

// detectPingType returns true if `text` is a JSON ping frame
// (`"type":"ping"`).
func detectPingType(text []byte) bool {
	return strings.Contains(string(text), `"type":"ping"`) ||
		strings.Contains(string(text), `"type": "ping"`)
}

func jsonEscape(s string) string {
	r := strings.NewReplacer(`\`, `\\`, `"`, `\"`)
	return r.Replace(s)
}

var wsUpgrader = websocket.Upgrader{
	ReadBufferSize:  4096,
	WriteBufferSize: 4096,
	// Allow any origin — internal benchmark only.
	CheckOrigin: func(r *http.Request) bool { return true },
}

// handleWS is the per-connection loop. One goroutine per connection
// (Go's scheduler maps M goroutines to N OS threads = no per-conn
// thread cost). Counterpart to Rust's `handle_ws_conn` and Dart's
// per-session microtask queue.
func handleWS(w http.ResponseWriter, r *http.Request) {
	conn, err := wsUpgrader.Upgrade(w, r, nil)
	if err != nil {
		wsHandshakeFailTotal.Inc()
		return
	}
	defer func() {
		_ = conn.Close()
		wsClosedTotal.Inc()
		wsActive.Dec()
	}()
	wsConnsTotal.Inc()
	wsActive.Inc()

	// Mirror dd-rust-wss-server's read deadline shape — generous so
	// idle keepalive doesn't kick clients during slow-ramp benchmarks.
	conn.SetReadLimit(64 * 1024)
	conn.SetReadDeadline(time.Time{})

	for {
		mt, data, err := conn.ReadMessage()
		if err != nil {
			return
		}
		wsMsgsInTotal.Inc()

		switch mt {
		case websocket.TextMessage:
			if detectPingType(data) {
				if id, ok := extractIDFromJSON(data); ok {
					ts := time.Now().UnixMilli()
					reply := fmt.Sprintf(`{"type":"pong","id":"%s","ts":%d}`, jsonEscape(id), ts)
					if err := conn.WriteMessage(websocket.TextMessage, []byte(reply)); err != nil {
						return
					}
					wsMsgsOutTotal.Inc()
					continue
				}
			}
			if id, ok := extractIDFromJSON(data); ok {
				reply := fmt.Sprintf(`{"ok":true,"result":{"id":"%s"}}`, jsonEscape(id))
				if err := conn.WriteMessage(websocket.TextMessage, []byte(reply)); err != nil {
					return
				}
				wsMsgsOutTotal.Inc()
				continue
			}
			if string(data) == "ping" {
				ts := time.Now().UnixMilli()
				reply := fmt.Sprintf(`{"type":"pong","ts":%d}`, ts)
				if err := conn.WriteMessage(websocket.TextMessage, []byte(reply)); err != nil {
					return
				}
				wsMsgsOutTotal.Inc()
			}
		case websocket.BinaryMessage:
			// drop silently
		case websocket.PingMessage:
			_ = conn.WriteMessage(websocket.PongMessage, nil)
		}
	}
}

// ---- main ------------------------------------------------------------

func runAcceptor(id int, ln net.Listener, mux http.Handler) {
	wsAcceptors.WithLabelValues(strconv.Itoa(id)).Set(1)
	srv := &http.Server{
		Handler:           mux,
		ReadTimeout:       30 * time.Second,
		WriteTimeout:      30 * time.Second,
		IdleTimeout:       0,
		ReadHeaderTimeout: 10 * time.Second,
	}
	if err := srv.Serve(ln); err != nil && !errors.Is(err, http.ErrServerClosed) {
		log.Printf("acceptor %d serve: %v", id, err)
	}
}

func startAdminServer(addr string) {
	mux := http.NewServeMux()
	mux.Handle("/metrics", promhttp.Handler())
	mux.HandleFunc("/healthz", func(w http.ResponseWriter, r *http.Request) {
		fmt.Fprintln(w, "ok")
	})
	mux.HandleFunc("/readyz", func(w http.ResponseWriter, r *http.Request) {
		if !ready.Load() {
			http.Error(w, "not ready", http.StatusServiceUnavailable)
			return
		}
		fmt.Fprintln(w, "ready")
	})
	srv := &http.Server{Addr: addr, Handler: telemetry.Handler(mux, "dd-go-wss-server-admin"), ReadHeaderTimeout: 5 * time.Second}
	log.Printf("admin server listening on %s", addr)
	if err := srv.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
		log.Fatalf("admin: %v", err)
	}
}

func main() {
	host := envOr("HOST", "0.0.0.0")
	wsPort := envInt("WS_PORT", 8107)
	adminPort := envInt("ADMIN_PORT", 8108)
	shards := envInt("WS_GATEWAY_SHARDS", 8)

	log.Printf("dd-go-wss-server starting host=%s ws_port=%d admin_port=%d shards=%d gomaxprocs=%d",
		host, wsPort, adminPort, shards, runtime.GOMAXPROCS(0))

	if shutdown, err := telemetry.Init(context.Background(), "dd-go-wss-server"); err != nil {
		log.Printf("telemetry init failed (continuing without traces): %v", err)
	} else {
		defer func() { _ = shutdown(context.Background()) }()
	}

	go startAdminServer(fmt.Sprintf(":%d", adminPort))

	wsAddr := fmt.Sprintf("%s:%d", host, wsPort)
	cfg := reuseportListenConfig()

	mux := http.NewServeMux()
	mux.HandleFunc("/", handleWS)

	for i := 0; i < shards; i++ {
		ln, err := cfg.Listen(context.Background(), "tcp", wsAddr)
		if err != nil {
			log.Fatalf("acceptor %d listen %s: %v", i, wsAddr, err)
		}
		log.Printf("acceptor %d bound %s (SO_REUSEPORT)", i, wsAddr)
		go runAcceptor(i, ln, telemetry.Handler(mux, "dd-go-wss-server"))
	}

	ready.Store(true)

	stop := make(chan os.Signal, 1)
	signal.Notify(stop, syscall.SIGINT, syscall.SIGTERM)
	<-stop
	log.Printf("shutdown signal received; exiting")
}

