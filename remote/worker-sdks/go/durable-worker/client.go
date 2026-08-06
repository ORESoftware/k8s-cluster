// Package durableworker provides a dependency-free client and long-lived worker
// loop for the ORESoftware durable-worker runtime.
package durableworker

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math/rand"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"sync"
	"time"
)

const (
	defaultAuthHeader       = "X-Worker-Auth"
	defaultUserAgent        = "oresoftware-durable-worker-go/0.1.0"
	defaultTimeout          = 30 * time.Second
	defaultMaxRetries       = 3
	defaultInitialBackoff   = 200 * time.Millisecond
	defaultMaxBackoff       = 5 * time.Second
	defaultMaxResponseBytes = int64(2 * 1024 * 1024)
)

var transientStatuses = map[int]struct{}{
	http.StatusRequestTimeout:      {},
	http.StatusTooEarly:            {},
	http.StatusTooManyRequests:     {},
	http.StatusInternalServerError: {},
	http.StatusBadGateway:          {},
	http.StatusServiceUnavailable:  {},
	http.StatusGatewayTimeout:      {},
}

// JSON is the runtime's language-neutral object payload.
type JSON map[string]any

// Error is a structured durable-worker protocol or transport error.
type Error struct {
	Code      string
	Message   string
	Status    int
	Retryable bool
	Cause     error
}

func (e *Error) Error() string {
	if e == nil {
		return "<nil>"
	}
	if e.Status > 0 {
		return fmt.Sprintf("durable-worker %s (HTTP %d): %s", e.Code, e.Status, e.Message)
	}
	return fmt.Sprintf("durable-worker %s: %s", e.Code, e.Message)
}

func (e *Error) Unwrap() error { return e.Cause }

// LeaseLostError indicates that the caller no longer owns the step lease.
// Terminal writes under the stale lease must be suppressed.
type LeaseLostError struct{ Protocol *Error }

func (e *LeaseLostError) Error() string {
	if e == nil || e.Protocol == nil {
		return "durable-worker lease lost"
	}
	return e.Protocol.Error()
}

func (e *LeaseLostError) Unwrap() error {
	if e == nil {
		return nil
	}
	return e.Protocol
}

// IsLeaseLost reports whether err means the current lease generation was fenced.
func IsLeaseLost(err error) bool {
	var target *LeaseLostError
	return errors.As(err, &target)
}

// ClientOptions configures a Client. Zero-valued fields use safe defaults.
type ClientOptions struct {
	AuthHeader       string
	Timeout          time.Duration
	MaxRetries       int
	DisableRetries   bool
	InitialBackoff   time.Duration
	MaxBackoff       time.Duration
	MaxResponseBytes int64
	Transport        http.RoundTripper
	Sleep            func(context.Context, time.Duration) error
	RandomSource     rand.Source
}

// Client talks to the durable-worker HTTP API.
type Client struct {
	baseURL          *url.URL
	authSecret       string
	authHeader       string
	timeout          time.Duration
	maxRetries       int
	initialBackoff   time.Duration
	maxBackoff       time.Duration
	maxResponseBytes int64
	httpClient       *http.Client
	sleep            func(context.Context, time.Duration) error
	random           *rand.Rand
	randomMu         sync.Mutex
}

// NewClient creates a protocol client. Redirects are deliberately refused so
// the configured authorization header cannot be forwarded to another origin.
func NewClient(baseURL, authSecret string, options ClientOptions) (*Client, error) {
	parsed, err := url.Parse(baseURL)
	if err != nil {
		return nil, fmt.Errorf("parse base URL: %w", err)
	}
	if parsed.Scheme != "http" && parsed.Scheme != "https" {
		return nil, errors.New("base URL must use http or https")
	}
	if parsed.Host == "" || parsed.User != nil || parsed.RawQuery != "" || parsed.Fragment != "" {
		return nil, errors.New("base URL must be absolute and must not contain credentials, query, or fragment")
	}
	if strings.TrimSpace(authSecret) == "" || strings.ContainsAny(authSecret, "\r\n") {
		return nil, errors.New("auth secret must be a non-empty single-line value")
	}

	authHeader := options.AuthHeader
	if authHeader == "" {
		authHeader = defaultAuthHeader
	}
	if strings.TrimSpace(authHeader) != authHeader || strings.ContainsAny(authHeader, " \t\r\n") {
		return nil, errors.New("auth header must be a non-empty HTTP token")
	}

	timeout := options.Timeout
	if timeout == 0 {
		timeout = defaultTimeout
	}
	if timeout < 0 {
		return nil, errors.New("timeout must be positive")
	}
	maxRetries := options.MaxRetries
	if options.DisableRetries {
		maxRetries = 0
	} else if maxRetries == 0 {
		maxRetries = defaultMaxRetries
	}
	if maxRetries < 0 {
		return nil, errors.New("max retries must be non-negative")
	}
	initialBackoff := options.InitialBackoff
	if initialBackoff == 0 {
		initialBackoff = defaultInitialBackoff
	}
	if initialBackoff < 0 {
		return nil, errors.New("initial backoff must be non-negative")
	}
	maxBackoff := options.MaxBackoff
	if maxBackoff == 0 {
		maxBackoff = defaultMaxBackoff
	}
	if maxBackoff < initialBackoff {
		return nil, errors.New("max backoff must be greater than or equal to initial backoff")
	}
	maxResponseBytes := options.MaxResponseBytes
	if maxResponseBytes == 0 {
		maxResponseBytes = defaultMaxResponseBytes
	}
	if maxResponseBytes < 1 {
		return nil, errors.New("max response bytes must be positive")
	}

	transport := options.Transport
	if transport == nil {
		transport = http.DefaultTransport
	}
	sleep := options.Sleep
	if sleep == nil {
		sleep = sleepContext
	}
	randomSource := options.RandomSource
	if randomSource == nil {
		randomSource = rand.NewSource(time.Now().UnixNano())
	}

	parsed.Path = strings.TrimRight(parsed.Path, "/")
	return &Client{
		baseURL:          parsed,
		authSecret:       authSecret,
		authHeader:       authHeader,
		timeout:          timeout,
		maxRetries:       maxRetries,
		initialBackoff:   initialBackoff,
		maxBackoff:       maxBackoff,
		maxResponseBytes: maxResponseBytes,
		httpClient: &http.Client{
			Transport: transport,
			Timeout:   timeout,
			CheckRedirect: func(_ *http.Request, _ []*http.Request) error {
				return http.ErrUseLastResponse
			},
		},
		sleep:  sleep,
		random: rand.New(randomSource),
	}, nil
}

func sleepContext(ctx context.Context, duration time.Duration) error {
	if duration <= 0 {
		return nil
	}
	timer := time.NewTimer(duration)
	defer timer.Stop()
	select {
	case <-ctx.Done():
		return ctx.Err()
	case <-timer.C:
		return nil
	}
}

func (c *Client) request(
	ctx context.Context,
	method string,
	path string,
	payload any,
	idempotent bool,
	leaseSensitive bool,
) (JSON, error) {
	if !strings.HasPrefix(path, "/") {
		return nil, errors.New("request path must be absolute")
	}

	var bodyBytes []byte
	var err error
	if payload != nil {
		bodyBytes, err = json.Marshal(payload)
		if err != nil {
			return nil, fmt.Errorf("encode request payload: %w", err)
		}
	}

	attempts := 1
	if idempotent {
		attempts += c.maxRetries
	}
	for attempt := 0; attempt < attempts; attempt++ {
		attemptCtx := ctx
		var cancel context.CancelFunc
		if c.timeout > 0 {
			attemptCtx, cancel = context.WithTimeout(ctx, c.timeout)
		}
		req, requestErr := http.NewRequestWithContext(attemptCtx, method, c.baseURL.String()+path, bytes.NewReader(bodyBytes))
		if requestErr != nil {
			if cancel != nil {
				cancel()
			}
			return nil, fmt.Errorf("build request: %w", requestErr)
		}
		req.Header.Set(c.authHeader, c.authSecret)
		req.Header.Set("Accept", "application/json")
		req.Header.Set("User-Agent", defaultUserAgent)
		if payload != nil {
			req.Header.Set("Content-Type", "application/json")
		}

		response, requestErr := c.httpClient.Do(req)
		if requestErr != nil {
			if cancel != nil {
				cancel()
			}
			if ctx.Err() != nil {
				return nil, ctx.Err()
			}
			if idempotent && attempt+1 < attempts {
				if sleepErr := c.sleep(ctx, c.backoff(attempt, "")); sleepErr != nil {
					return nil, sleepErr
				}
				continue
			}
			return nil, &Error{
				Code:      "transport_error",
				Message:   fmt.Sprintf("durable-worker request failed: %v", requestErr),
				Retryable: true,
				Cause:     requestErr,
			}
		}

		decoded, decodeErr := c.decodeResponse(response, response.StatusCode >= 200 && response.StatusCode < 300)
		if cancel != nil {
			cancel()
		}
		if decodeErr != nil {
			return nil, decodeErr
		}
		if response.StatusCode >= 200 && response.StatusCode < 300 {
			return decoded, nil
		}

		protocolErr := httpError(response.StatusCode, decoded, leaseSensitive)
		if idempotent && attempt+1 < attempts && isTransientStatus(response.StatusCode) && protocolErrRetryable(protocolErr) {
			if sleepErr := c.sleep(ctx, c.backoff(attempt, response.Header.Get("Retry-After"))); sleepErr != nil {
				return nil, sleepErr
			}
			continue
		}
		return nil, protocolErr
	}
	return nil, errors.New("durable-worker retry loop exhausted unexpectedly")
}

func (c *Client) decodeResponse(response *http.Response, strictJSON bool) (JSON, error) {
	defer response.Body.Close()
	if response.ContentLength > c.maxResponseBytes {
		_, _ = io.Copy(io.Discard, io.LimitReader(response.Body, 32*1024))
		return nil, &Error{Code: "response_too_large", Message: "durable-worker response exceeds configured limit"}
	}
	reader := io.LimitReader(response.Body, c.maxResponseBytes+1)
	body, err := io.ReadAll(reader)
	if err != nil {
		return nil, &Error{Code: "response_read_error", Message: err.Error(), Retryable: true, Cause: err}
	}
	if int64(len(body)) > c.maxResponseBytes {
		return nil, &Error{Code: "response_too_large", Message: "durable-worker response exceeds configured limit"}
	}
	if len(body) == 0 {
		return JSON{}, nil
	}
	var decoded JSON
	if err := json.Unmarshal(body, &decoded); err != nil {
		if !strictJSON {
			return JSON{}, nil
		}
		return nil, &Error{Code: "invalid_response", Message: "durable-worker returned a non-object JSON response", Cause: err}
	}
	return decoded, nil
}

func (c *Client) backoff(attempt int, retryAfter string) time.Duration {
	if retryAfter != "" {
		if seconds, err := strconv.ParseFloat(strings.TrimSpace(retryAfter), 64); err == nil && seconds >= 0 {
			duration := time.Duration(seconds * float64(time.Second))
			if duration > c.maxBackoff {
				return c.maxBackoff
			}
			return duration
		}
	}
	ceiling := c.initialBackoff
	for index := 0; index < attempt; index++ {
		if ceiling >= c.maxBackoff/2 {
			ceiling = c.maxBackoff
			break
		}
		ceiling *= 2
	}
	if ceiling > c.maxBackoff {
		ceiling = c.maxBackoff
	}
	if ceiling <= 0 {
		return 0
	}
	floor := ceiling / 2
	c.randomMu.Lock()
	jitter := time.Duration(c.random.Int63n(int64(ceiling-floor) + 1))
	c.randomMu.Unlock()
	return floor + jitter
}

func isTransientStatus(status int) bool {
	_, ok := transientStatuses[status]
	return ok
}

func protocolErrRetryable(err error) bool {
	var protocolErr *Error
	if errors.As(err, &protocolErr) {
		return protocolErr.Retryable
	}
	return false
}

func httpError(status int, body JSON, leaseSensitive bool) error {
	message := stringValue(body["message"])
	if message == "" {
		message = fmt.Sprintf("durable-worker returned HTTP %d", status)
	}
	code := stringValue(body["code"])
	if code == "" {
		code = "http_error"
	}
	retryable, ok := body["retryable"].(bool)
	if !ok {
		retryable = isTransientStatus(status)
	}
	protocolErr := &Error{Code: code, Message: message, Status: status, Retryable: retryable}
	if leaseSensitive && (status == http.StatusNotFound || status == http.StatusConflict) {
		return &LeaseLostError{Protocol: protocolErr}
	}
	return protocolErr
}

func stringValue(value any) string {
	if value == nil {
		return ""
	}
	if text, ok := value.(string); ok {
		return text
	}
	return fmt.Sprint(value)
}

func segment(value string) (string, error) {
	if value == "" {
		return "", errors.New("path identifier must be non-empty")
	}
	return url.PathEscape(value), nil
}

// Lease identifies one authoritative worker assignment generation.
type Lease struct {
	WorkerID        string `json:"workerId"`
	LeaseToken      string `json:"leaseToken"`
	LeaseGeneration int64  `json:"leaseGeneration"`
}

func (c *Client) SubmitTask(ctx context.Context, task JSON) (JSON, error) {
	_, idempotent := task["idempotencyKey"].(string)
	return c.request(ctx, http.MethodPost, "/api/v1/tasks", task, idempotent && stringValue(task["idempotencyKey"]) != "", false)
}

func (c *Client) SubmitRun(ctx context.Context, run JSON) (JSON, error) {
	_, idempotent := run["idempotencyKey"].(string)
	return c.request(ctx, http.MethodPost, "/api/v1/runs", run, idempotent && stringValue(run["idempotencyKey"]) != "", false)
}

func (c *Client) GetRun(ctx context.Context, runID string) (JSON, error) {
	encoded, err := segment(runID)
	if err != nil {
		return nil, err
	}
	return c.request(ctx, http.MethodGet, "/api/v1/runs/"+encoded, nil, true, false)
}

func (c *Client) SignalRun(ctx context.Context, runID, signalName string, payload JSON) (JSON, error) {
	encodedRun, err := segment(runID)
	if err != nil {
		return nil, err
	}
	encodedSignal, err := segment(signalName)
	if err != nil {
		return nil, err
	}
	if payload == nil {
		payload = JSON{}
	}
	return c.request(ctx, http.MethodPost, "/api/v1/runs/"+encodedRun+"/signals/"+encodedSignal, JSON{"payload": payload}, false, false)
}

func (c *Client) PauseRun(ctx context.Context, runID string) (JSON, error) {
	return c.runMutation(ctx, runID, "pause")
}

func (c *Client) ResumeRun(ctx context.Context, runID string) (JSON, error) {
	return c.runMutation(ctx, runID, "resume")
}

func (c *Client) CancelRun(ctx context.Context, runID string) (JSON, error) {
	return c.runMutation(ctx, runID, "cancel")
}

func (c *Client) runMutation(ctx context.Context, runID, operation string) (JSON, error) {
	encoded, err := segment(runID)
	if err != nil {
		return nil, err
	}
	return c.request(ctx, http.MethodPost, "/api/v1/runs/"+encoded+"/"+operation, JSON{}, true, false)
}

func (c *Client) RegisterWorker(ctx context.Context, registration JSON) (JSON, error) {
	return c.request(ctx, http.MethodPost, "/api/v1/workers/register", registration, true, false)
}

func (c *Client) HeartbeatWorker(ctx context.Context, workerID string, drain *bool) (JSON, error) {
	encoded, err := segment(workerID)
	if err != nil {
		return nil, err
	}
	payload := JSON{}
	if drain != nil {
		payload["drain"] = *drain
	}
	return c.request(ctx, http.MethodPost, "/api/v1/workers/"+encoded+"/heartbeat", payload, true, false)
}

func (c *Client) PollWorker(ctx context.Context, workerID string, wait time.Duration) (JSON, error) {
	encoded, err := segment(workerID)
	if err != nil {
		return nil, err
	}
	if wait < 0 {
		return nil, errors.New("poll wait must be non-negative")
	}
	waitMS := wait.Milliseconds()
	path := "/api/v1/workers/" + encoded + "/poll?waitMs=" + strconv.FormatInt(waitMS, 10)
	return c.request(ctx, http.MethodPost, path, JSON{}, false, false)
}

func (c *Client) StartStep(ctx context.Context, stepID string, lease Lease) (JSON, error) {
	return c.leaseMutation(ctx, stepID, "start", lease)
}

func (c *Client) HeartbeatStep(ctx context.Context, stepID string, lease Lease) (JSON, error) {
	return c.leaseMutation(ctx, stepID, "heartbeat", lease)
}

func (c *Client) AppendStepOutput(ctx context.Context, stepID string, lease Lease, chunkID, chunk, stream string, final bool) (JSON, error) {
	encoded, err := segment(stepID)
	if err != nil {
		return nil, err
	}
	if stream == "" {
		stream = "progress"
	}
	payload := JSON{
		"workerId":        lease.WorkerID,
		"leaseToken":      lease.LeaseToken,
		"leaseGeneration": lease.LeaseGeneration,
		"chunkId":         chunkID,
		"chunk":           chunk,
		"stream":          stream,
		"finalChunk":      final,
	}
	return c.request(ctx, http.MethodPost, "/api/v1/steps/"+encoded+"/output", payload, true, true)
}

func (c *Client) CompleteStep(ctx context.Context, stepID string, lease Lease, result JSON) (JSON, error) {
	encoded, err := segment(stepID)
	if err != nil {
		return nil, err
	}
	if result == nil {
		result = JSON{}
	}
	payload := JSON{
		"workerId":        lease.WorkerID,
		"leaseToken":      lease.LeaseToken,
		"leaseGeneration": lease.LeaseGeneration,
		"result":          result,
	}
	return c.request(ctx, http.MethodPost, "/api/v1/steps/"+encoded+"/complete", payload, true, true)
}

func (c *Client) FailStep(ctx context.Context, stepID string, lease Lease, code, message string, retryable bool) (JSON, error) {
	encoded, err := segment(stepID)
	if err != nil {
		return nil, err
	}
	payload := JSON{
		"workerId":        lease.WorkerID,
		"leaseToken":      lease.LeaseToken,
		"leaseGeneration": lease.LeaseGeneration,
		"code":            code,
		"message":         message,
		"retryable":       retryable,
	}
	return c.request(ctx, http.MethodPost, "/api/v1/steps/"+encoded+"/fail", payload, true, true)
}

func (c *Client) leaseMutation(ctx context.Context, stepID, operation string, lease Lease) (JSON, error) {
	encoded, err := segment(stepID)
	if err != nil {
		return nil, err
	}
	return c.request(ctx, http.MethodPost, "/api/v1/steps/"+encoded+"/"+operation, lease, true, true)
}
