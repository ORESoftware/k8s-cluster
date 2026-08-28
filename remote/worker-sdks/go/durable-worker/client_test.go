package durableworker

import (
	"context"
	"errors"
	"io"
	"math/rand"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"
)

type roundTripFunc func(*http.Request) (*http.Response, error)

func (function roundTripFunc) RoundTrip(request *http.Request) (*http.Response, error) {
	return function(request)
}

func response(status int, body string, headers http.Header) *http.Response {
	if headers == nil {
		headers = make(http.Header)
	}
	return &http.Response{
		StatusCode:    status,
		Header:        headers,
		Body:          io.NopCloser(strings.NewReader(body)),
		ContentLength: int64(len(body)),
	}
}

func TestSubmitRetriesOnlyWithIdempotencyKey(t *testing.T) {
	t.Parallel()
	var mu sync.Mutex
	requests := 0
	var auth string
	transport := roundTripFunc(func(request *http.Request) (*http.Response, error) {
		mu.Lock()
		defer mu.Unlock()
		requests++
		auth = request.Header.Get("X-Worker-Auth")
		if strings.Contains(request.URL.String(), "secret-value") {
			t.Fatal("secret leaked into URL")
		}
		if requests == 1 {
			return response(503, `{"code":"busy","message":"busy","retryable":true}`, nil), nil
		}
		return response(202, `{"runId":"run-1","status":"pending"}`, nil), nil
	})
	var sleeps []time.Duration
	client, err := NewClient("https://workers.example.test", "secret-value", ClientOptions{
		Transport:      transport,
		InitialBackoff: time.Nanosecond,
		MaxBackoff:     time.Nanosecond,
		RandomSource:   rand.NewSource(1),
		Sleep: func(_ context.Context, duration time.Duration) error {
			sleeps = append(sleeps, duration)
			return nil
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	result, err := client.SubmitTask(context.Background(), JSON{
		"idempotencyKey": "stable",
		"taskType":       "demo",
		"input":          JSON{},
	})
	if err != nil {
		t.Fatal(err)
	}
	if result["runId"] != "run-1" {
		t.Fatalf("unexpected result: %#v", result)
	}
	if requests != 2 || len(sleeps) != 1 {
		t.Fatalf("expected one retry, requests=%d sleeps=%d", requests, len(sleeps))
	}
	if auth != "secret-value" {
		t.Fatalf("authorization header missing: %q", auth)
	}

	requests = 0
	transport = roundTripFunc(func(_ *http.Request) (*http.Response, error) {
		requests++
		return response(503, `{"code":"busy","message":"busy","retryable":true}`, nil), nil
	})
	client, err = NewClient("https://workers.example.test", "secret-value", ClientOptions{Transport: transport})
	if err != nil {
		t.Fatal(err)
	}
	_, err = client.SubmitTask(context.Background(), JSON{"taskType": "demo", "input": JSON{}})
	if err == nil {
		t.Fatal("expected submission failure")
	}
	if requests != 1 {
		t.Fatalf("unbound submission was retried %d times", requests)
	}
}

func TestPollIsNotRetriedAfterAmbiguousTransportFailure(t *testing.T) {
	t.Parallel()
	requests := 0
	client, err := NewClient("https://workers.example.test", "secret", ClientOptions{
		Transport: roundTripFunc(func(_ *http.Request) (*http.Response, error) {
			requests++
			return nil, errors.New("connection reset")
		}),
		Sleep: func(context.Context, time.Duration) error { return nil },
	})
	if err != nil {
		t.Fatal(err)
	}
	_, err = client.PollWorker(context.Background(), "worker-1", time.Second)
	if err == nil {
		t.Fatal("expected transport error")
	}
	var protocolErr *Error
	if !errors.As(err, &protocolErr) || !protocolErr.Retryable {
		t.Fatalf("expected retryable transport error, got %T: %v", err, err)
	}
	if requests != 1 {
		t.Fatalf("ambiguous poll was retried %d times", requests)
	}
}

func TestRedirectIsRefusedWithoutForwardingAuthorization(t *testing.T) {
	t.Parallel()
	var evilHits int
	var evilAuth string
	evil := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		evilHits++
		evilAuth = request.Header.Get("X-Worker-Auth")
		writer.Header().Set("Content-Type", "application/json")
		_, _ = writer.Write([]byte(`{"unexpected":true}`))
	}))
	defer evil.Close()

	origin := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		http.Redirect(writer, request, evil.URL+"/steal", http.StatusFound)
	}))
	defer origin.Close()

	client, err := NewClient(origin.URL, "top-secret", ClientOptions{})
	if err != nil {
		t.Fatal(err)
	}
	_, err = client.GetRun(context.Background(), "run-1")
	if err == nil {
		t.Fatal("expected redirect response to fail")
	}
	var protocolErr *Error
	if !errors.As(err, &protocolErr) || protocolErr.Status != http.StatusFound {
		t.Fatalf("expected HTTP 302 protocol error, got %T: %v", err, err)
	}
	if evilHits != 0 || evilAuth != "" {
		t.Fatalf("redirect target received request/auth: hits=%d auth=%q", evilHits, evilAuth)
	}
}

func TestResponseLimitAndInvalidJSON(t *testing.T) {
	t.Parallel()
	client, err := NewClient("https://workers.example.test", "secret", ClientOptions{
		MaxResponseBytes: 16,
		Transport: roundTripFunc(func(_ *http.Request) (*http.Response, error) {
			return response(200, `{"payload":"this is deliberately too large"}`, nil), nil
		}),
	})
	if err != nil {
		t.Fatal(err)
	}
	_, err = client.GetRun(context.Background(), "run-1")
	var protocolErr *Error
	if !errors.As(err, &protocolErr) || protocolErr.Code != "response_too_large" {
		t.Fatalf("expected response_too_large, got %T: %v", err, err)
	}

	client, err = NewClient("https://workers.example.test", "secret", ClientOptions{
		Transport: roundTripFunc(func(_ *http.Request) (*http.Response, error) {
			return response(200, `not-json`, nil), nil
		}),
	})
	if err != nil {
		t.Fatal(err)
	}
	_, err = client.GetRun(context.Background(), "run-1")
	if !errors.As(err, &protocolErr) || protocolErr.Code != "invalid_response" {
		t.Fatalf("expected invalid_response, got %T: %v", err, err)
	}
}

func TestLeaseConflictBecomesLeaseLost(t *testing.T) {
	t.Parallel()
	client, err := NewClient("https://workers.example.test", "secret", ClientOptions{
		DisableRetries: true,
		Transport: roundTripFunc(func(_ *http.Request) (*http.Response, error) {
			return response(409, `{"code":"state_conflict","message":"stale lease generation","retryable":true}`, nil), nil
		}),
	})
	if err != nil {
		t.Fatal(err)
	}
	_, err = client.CompleteStep(context.Background(), "step-1", Lease{WorkerID: "worker", LeaseToken: "token", LeaseGeneration: 3}, JSON{})
	if !IsLeaseLost(err) {
		t.Fatalf("expected lease loss, got %T: %v", err, err)
	}
	var protocolErr *Error
	if !errors.As(err, &protocolErr) || protocolErr.Status != 409 {
		t.Fatalf("lease loss should unwrap protocol error: %T %v", err, err)
	}
}

func TestCustomAuthHeaderAndRetryAfter(t *testing.T) {
	t.Parallel()
	var requests int
	var observedHeader string
	var slept time.Duration
	client, err := NewClient("https://workers.example.test/api", "secret", ClientOptions{
		AuthHeader: "Authorization",
		Transport: roundTripFunc(func(request *http.Request) (*http.Response, error) {
			requests++
			observedHeader = request.Header.Get("Authorization")
			if requests == 1 {
				headers := make(http.Header)
				headers.Set("Retry-After", "0.25")
				return response(429, `{"code":"rate_limited","message":"later","retryable":true}`, headers), nil
			}
			return response(200, `{"ok":true}`, nil), nil
		}),
		Sleep: func(_ context.Context, duration time.Duration) error {
			slept = duration
			return nil
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	_, err = client.PauseRun(context.Background(), "run/with/slash")
	if err != nil {
		t.Fatal(err)
	}
	if observedHeader != "secret" || slept != 250*time.Millisecond {
		t.Fatalf("header/backoff mismatch: header=%q sleep=%v", observedHeader, slept)
	}
}

func TestSignalIsNotRetriedWithoutProtocolIdentity(t *testing.T) {
	t.Parallel()
	requests := 0
	client, err := NewClient("https://workers.example.test", "secret", ClientOptions{
		Transport: roundTripFunc(func(_ *http.Request) (*http.Response, error) {
			requests++
			return nil, errors.New("connection reset after write")
		}),
		Sleep: func(context.Context, time.Duration) error { return nil },
	})
	if err != nil {
		t.Fatal(err)
	}
	_, err = client.SignalRun(context.Background(), "run-1", "approval", JSON{"approved": true})
	if err == nil {
		t.Fatal("expected ambiguous signal failure")
	}
	if requests != 1 {
		t.Fatalf("signal was retried %d times", requests)
	}
}

func TestClientRejectsCredentialBearingBaseURLAndMultilineSecret(t *testing.T) {
	t.Parallel()
	for _, testCase := range []struct {
		baseURL string
		secret  string
	}{
		{baseURL: "https://user:pass@workers.example.test", secret: "secret"},
		{baseURL: "https://workers.example.test?token=secret", secret: "secret"},
		{baseURL: "https://workers.example.test#fragment", secret: "secret"},
		{baseURL: "https://workers.example.test", secret: "secret\nInjected: value"},
	} {
		if _, err := NewClient(testCase.baseURL, testCase.secret, ClientOptions{}); err == nil {
			t.Fatalf("unsafe client configuration accepted: base=%q secret=%q", testCase.baseURL, testCase.secret)
		}
	}
}
