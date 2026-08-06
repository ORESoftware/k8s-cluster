package durableworker

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

type recordedCall struct {
	name    string
	payload JSON
}

type fakeWorkerClient struct {
	mu                 sync.Mutex
	assignments        []JSON
	pollErr            error
	fenceHeartbeat     bool
	fenceComplete      bool
	calls              []recordedCall
	pollCount          int
	stepHeartbeatCount int
}

func (client *fakeWorkerClient) record(name string, payload JSON) {
	client.mu.Lock()
	client.calls = append(client.calls, recordedCall{name: name, payload: cloneJSON(payload)})
	client.mu.Unlock()
}

func (client *fakeWorkerClient) RegisterWorker(_ context.Context, payload JSON) (JSON, error) {
	client.record("register", payload)
	return JSON{"workerId": payload["workerId"]}, nil
}

func (client *fakeWorkerClient) HeartbeatWorker(_ context.Context, workerID string, drain *bool) (JSON, error) {
	value := false
	if drain != nil {
		value = *drain
	}
	client.record("worker-heartbeat", JSON{"workerId": workerID, "drain": value})
	return JSON{"ok": true}, nil
}

func (client *fakeWorkerClient) PollWorker(_ context.Context, workerID string, wait time.Duration) (JSON, error) {
	client.mu.Lock()
	defer client.mu.Unlock()
	client.pollCount++
	if client.pollErr != nil {
		return nil, client.pollErr
	}
	if len(client.assignments) == 0 {
		return JSON{"assignment": nil, "retryAfterMs": 1}, nil
	}
	assignment := client.assignments[0]
	client.assignments = client.assignments[1:]
	client.calls = append(client.calls, recordedCall{name: "poll", payload: JSON{"workerId": workerID, "waitMs": wait.Milliseconds()}})
	return JSON{"assignment": cloneJSON(assignment), "retryAfterMs": 1}, nil
}

func (client *fakeWorkerClient) StartStep(_ context.Context, stepID string, lease Lease) (JSON, error) {
	client.record("start", leasePayload(stepID, lease))
	return JSON{"ok": true}, nil
}

func (client *fakeWorkerClient) HeartbeatStep(_ context.Context, stepID string, lease Lease) (JSON, error) {
	client.mu.Lock()
	client.stepHeartbeatCount++
	client.calls = append(client.calls, recordedCall{name: "step-heartbeat", payload: leasePayload(stepID, lease)})
	fenced := client.fenceHeartbeat
	client.mu.Unlock()
	if fenced {
		return nil, &LeaseLostError{Protocol: &Error{Code: "state_conflict", Message: "fenced", Status: 409}}
	}
	return JSON{"ok": true}, nil
}

func (client *fakeWorkerClient) AppendStepOutput(_ context.Context, stepID string, lease Lease, chunkID, chunk, stream string, final bool) (JSON, error) {
	payload := leasePayload(stepID, lease)
	payload["chunkId"] = chunkID
	payload["chunk"] = chunk
	payload["stream"] = stream
	payload["finalChunk"] = final
	client.record("output", payload)
	return JSON{"ok": true}, nil
}

func (client *fakeWorkerClient) CompleteStep(_ context.Context, stepID string, lease Lease, result JSON) (JSON, error) {
	payload := leasePayload(stepID, lease)
	payload["result"] = cloneJSON(result)
	client.record("complete", payload)
	if client.fenceComplete {
		return nil, &LeaseLostError{Protocol: &Error{Code: "state_conflict", Message: "fenced", Status: 409}}
	}
	return JSON{"ok": true}, nil
}

func (client *fakeWorkerClient) FailStep(_ context.Context, stepID string, lease Lease, code, message string, retryable bool) (JSON, error) {
	payload := leasePayload(stepID, lease)
	payload["code"] = code
	payload["message"] = message
	payload["retryable"] = retryable
	client.record("fail", payload)
	return JSON{"ok": true}, nil
}

func (client *fakeWorkerClient) snapshotCalls() []recordedCall {
	client.mu.Lock()
	defer client.mu.Unlock()
	result := make([]recordedCall, len(client.calls))
	copy(result, client.calls)
	return result
}

func leasePayload(stepID string, lease Lease) JSON {
	return JSON{
		"stepId":          stepID,
		"workerId":        lease.WorkerID,
		"leaseToken":      lease.LeaseToken,
		"leaseGeneration": lease.LeaseGeneration,
	}
}

func assignment(sequence int) JSON {
	return JSON{
		"runId":            fmt.Sprintf("run-%d", sequence),
		"stepId":           fmt.Sprintf("step-%d", sequence),
		"stepKey":          "task",
		"taskType":         "demo",
		"queue":            "default",
		"input":            JSON{"value": sequence},
		"attempt":          1,
		"leaseToken":       fmt.Sprintf("lease-%d", sequence),
		"leaseGeneration":  3,
		"fencingToken":     9,
		"leaseExpiresAtMs": time.Now().Add(30 * time.Second).UnixMilli(),
		"timeoutMs":        60_000,
		"affinityKey":      nil,
	}
}

func workerConfig(maxAssignments, slots int) WorkerConfig {
	return WorkerConfig{
		WorkerID:        "worker-1",
		Queues:          []string{"default"},
		Capabilities:    []string{"demo"},
		Slots:           slots,
		TTL:             50 * time.Millisecond,
		PollWait:        time.Millisecond,
		WorkerHeartbeat: 3 * time.Millisecond,
		StepHeartbeat:   2 * time.Millisecond,
		IdleSleep:       time.Millisecond,
		MaxAssignments:  maxAssignments,
	}
}

func TestWorkerStreamsProgressHeartbeatsAndCompletesSameGeneration(t *testing.T) {
	t.Parallel()
	client := &fakeWorkerClient{assignments: []JSON{assignment(1)}}
	summary, err := RunWorker(context.Background(), client, map[string]Handler{
		"demo": func(task *TaskContext) (JSON, error) {
			if task.FencingToken() != 9 || task.Input()["value"] != 1 {
				t.Fatalf("assignment context mismatch: %#v", task.Assignment())
			}
			if _, err := task.Emit("working", OutputOptions{}); err != nil {
				return nil, err
			}
			time.Sleep(8 * time.Millisecond)
			return JSON{"answer": 2}, nil
		},
	}, workerConfig(1, 1))
	if err != nil {
		t.Fatal(err)
	}
	if summary != (WorkerSummary{Accepted: 1, Completed: 1}) {
		t.Fatalf("unexpected summary: %#v", summary)
	}

	calls := client.snapshotCalls()
	var sawHeartbeat, sawDrain bool
	var output, complete JSON
	for _, call := range calls {
		switch call.name {
		case "step-heartbeat":
			sawHeartbeat = true
		case "output":
			output = call.payload
		case "complete":
			complete = call.payload
		case "worker-heartbeat":
			if call.payload["drain"] == true {
				sawDrain = true
			}
		}
	}
	if !sawHeartbeat || !sawDrain {
		t.Fatalf("heartbeat/drain missing: %#v", calls)
	}
	if output["chunkId"] != "step-1:3:1" || output["leaseGeneration"] != int64(3) {
		t.Fatalf("progress identity mismatch: %#v", output)
	}
	if complete["leaseGeneration"] != int64(3) {
		t.Fatalf("completion used stale generation: %#v", complete)
	}
	result := complete["result"].(JSON)
	if result["answer"] != 2 {
		t.Fatalf("completion result mismatch: %#v", result)
	}
}

func TestFencedHeartbeatCancelsHandlerAndSuppressesTerminalMutation(t *testing.T) {
	client := &fakeWorkerClient{assignments: []JSON{assignment(1)}, fenceHeartbeat: true}
	observedCancellation := make(chan error, 1)
	summary, err := RunWorker(context.Background(), client, map[string]Handler{
		"demo": func(task *TaskContext) (JSON, error) {
			select {
			case <-task.Context().Done():
				observedCancellation <- context.Cause(task.Context())
				return nil, task.RaiseIfCancelled()
			case <-time.After(time.Second):
				return nil, errors.New("lease cancellation was not observed")
			}
		},
	}, workerConfig(1, 1))
	if err != nil {
		t.Fatal(err)
	}
	if summary.Accepted != 1 || summary.LeaseLost != 1 || summary.Completed != 0 || summary.Failed != 0 {
		t.Fatalf("unexpected fenced summary: %#v", summary)
	}
	select {
	case cause := <-observedCancellation:
		if !IsLeaseLost(cause) {
			t.Fatalf("handler cancellation cause was not lease loss: %T %v", cause, cause)
		}
	default:
		t.Fatal("handler did not observe cancellation")
	}
	for _, call := range client.snapshotCalls() {
		if call.name == "complete" || call.name == "fail" {
			t.Fatalf("stale terminal mutation was sent: %#v", call)
		}
	}
}

func TestWorkerFailureAndMissingHandlerAreExplicit(t *testing.T) {
	t.Parallel()
	cases := []struct {
		name      string
		handlers  map[string]Handler
		code      string
		retryable bool
	}{
		{
			name: "classified",
			handlers: map[string]Handler{"demo": func(*TaskContext) (JSON, error) {
				return nil, &WorkerFailure{Code: "upstream_busy", Message: "try later", Retryable: true}
			}},
			code:      "upstream_busy",
			retryable: true,
		},
		{name: "missing", handlers: map[string]Handler{}, code: "handler_not_found", retryable: false},
		{
			name: "panic",
			handlers: map[string]Handler{"demo": func(*TaskContext) (JSON, error) {
				panic("boom")
			}},
			code:      "handler_panic",
			retryable: false,
		},
	}
	for _, testCase := range cases {
		testCase := testCase
		t.Run(testCase.name, func(t *testing.T) {
			t.Parallel()
			client := &fakeWorkerClient{assignments: []JSON{assignment(1)}}
			summary, err := RunWorker(context.Background(), client, testCase.handlers, workerConfig(1, 1))
			if err != nil {
				t.Fatal(err)
			}
			if summary.Failed != 1 || summary.Completed != 0 {
				t.Fatalf("unexpected failure summary: %#v", summary)
			}
			var failure JSON
			for _, call := range client.snapshotCalls() {
				if call.name == "fail" {
					failure = call.payload
				}
			}
			if failure["code"] != testCase.code || failure["retryable"] != testCase.retryable {
				t.Fatalf("failure classification mismatch: %#v", failure)
			}
			if testCase.code == "handler_panic" && !strings.Contains(failure["message"].(string), "boom") {
				t.Fatalf("panic detail absent: %#v", failure)
			}
		})
	}
}

func TestWorkerNeverPollsBeyondLocalSlots(t *testing.T) {
	client := &fakeWorkerClient{assignments: []JSON{assignment(1), assignment(2), assignment(3)}}
	var current atomic.Int64
	var maximum atomic.Int64
	started := make(chan struct{}, 3)
	release := make(chan struct{})

	done := make(chan struct{})
	var summary WorkerSummary
	var runErr error
	go func() {
		summary, runErr = RunWorker(context.Background(), client, map[string]Handler{
			"demo": func(*TaskContext) (JSON, error) {
				active := current.Add(1)
				for {
					observed := maximum.Load()
					if active <= observed || maximum.CompareAndSwap(observed, active) {
						break
					}
				}
				started <- struct{}{}
				<-release
				current.Add(-1)
				return JSON{"ok": true}, nil
			},
		}, workerConfig(3, 2))
		close(done)
	}()

	select {
	case <-started:
	case <-time.After(time.Second):
		t.Fatal("first handler did not start")
	}
	select {
	case <-started:
	case <-time.After(time.Second):
		t.Fatal("second handler did not start")
	}
	if maximum.Load() != 2 {
		t.Fatalf("expected exactly two concurrent handlers, got %d", maximum.Load())
	}
	client.mu.Lock()
	pollsBeforeRelease := client.pollCount
	client.mu.Unlock()
	if pollsBeforeRelease != 2 {
		t.Fatalf("worker polled past local slots: %d polls", pollsBeforeRelease)
	}
	close(release)
	select {
	case <-done:
	case <-time.After(time.Second):
		t.Fatal("worker did not drain")
	}
	if runErr != nil || summary.Completed != 3 || maximum.Load() > 2 {
		t.Fatalf("concurrency result mismatch: summary=%#v max=%d err=%v", summary, maximum.Load(), runErr)
	}
}

func TestAmbiguousPollStopsWorkerWithoutClientSideRetry(t *testing.T) {
	t.Parallel()
	pollErr := &Error{Code: "transport_error", Message: "connection reset", Retryable: true}
	client := &fakeWorkerClient{pollErr: pollErr}
	summary, err := RunWorker(context.Background(), client, map[string]Handler{}, workerConfig(1, 1))
	if !errors.Is(err, pollErr) {
		t.Fatalf("expected ambiguous poll error, got %T: %v", err, err)
	}
	if summary.Accepted != 0 {
		t.Fatalf("ambiguous poll accepted local work: %#v", summary)
	}
	client.mu.Lock()
	polls := client.pollCount
	client.mu.Unlock()
	if polls != 1 {
		t.Fatalf("worker repeated ambiguous poll %d times", polls)
	}
}

func TestCompletionFenceIsCountedWithoutFollowupFailure(t *testing.T) {
	t.Parallel()
	client := &fakeWorkerClient{assignments: []JSON{assignment(1)}, fenceComplete: true}
	summary, err := RunWorker(context.Background(), client, map[string]Handler{
		"demo": func(*TaskContext) (JSON, error) { return JSON{"ok": true}, nil },
	}, workerConfig(1, 1))
	if err != nil {
		t.Fatal(err)
	}
	if summary.LeaseLost != 1 || summary.Completed != 0 || summary.Failed != 0 {
		t.Fatalf("completion fence summary mismatch: %#v", summary)
	}
	for _, call := range client.snapshotCalls() {
		if call.name == "fail" {
			t.Fatalf("completion fence triggered stale failure: %#v", call)
		}
	}
}
