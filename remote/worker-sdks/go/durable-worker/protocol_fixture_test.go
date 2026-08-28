package durableworker

import (
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"sort"
	"testing"
)

type protocolFixture struct {
	Version                   int      `json:"version"`
	Delivery                  string   `json:"delivery"`
	EffectSafety              []string `json:"effectSafety"`
	TransientStatuses         []int    `json:"transientStatuses"`
	LeaseLostStatuses         []int    `json:"leaseLostStatuses"`
	NeverRetryWithoutIdentity []string `json:"neverRetryWithoutIdentity"`
	ProgressChunkID           string   `json:"progressChunkId"`
	Assignment                JSON     `json:"assignment"`
}

func loadProtocolFixture(t *testing.T) protocolFixture {
	t.Helper()
	_, sourceFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("cannot locate protocol fixture test")
	}
	path := filepath.Clean(filepath.Join(filepath.Dir(sourceFile), "..", "..", "fixtures", "durable-worker-protocol-v1.json"))
	payload, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read shared protocol fixture %s: %v", path, err)
	}
	var fixture protocolFixture
	if err := json.Unmarshal(payload, &fixture); err != nil {
		t.Fatalf("decode protocol fixture: %v", err)
	}
	return fixture
}

func TestSharedProtocolFixtureMatchesGoClientSafetyBoundaries(t *testing.T) {
	fixture := loadProtocolFixture(t)
	if fixture.Version != 1 || fixture.Delivery != "at-least-once" {
		t.Fatalf("unexpected protocol fixture identity: %#v", fixture)
	}
	if !reflect.DeepEqual(fixture.EffectSafety, []string{"idempotency-key", "fencing-token"}) {
		t.Fatalf("effect safety changed without SDK review: %#v", fixture.EffectSafety)
	}

	var actualTransient []int
	for status := range transientStatuses {
		actualTransient = append(actualTransient, status)
	}
	sort.Ints(actualTransient)
	expectedTransient := append([]int(nil), fixture.TransientStatuses...)
	sort.Ints(expectedTransient)
	if !reflect.DeepEqual(actualTransient, expectedTransient) {
		t.Fatalf("transient status drift: got %v want %v", actualTransient, expectedTransient)
	}

	for _, status := range fixture.LeaseLostStatuses {
		err := httpError(status, JSON{"code": "state_conflict", "message": "fenced"}, true)
		if !IsLeaseLost(err) {
			t.Fatalf("HTTP %d no longer fences lease-sensitive mutations: %T %v", status, err, err)
		}
		var protocolErr *Error
		if !errors.As(err, &protocolErr) || protocolErr.Status != status {
			t.Fatalf("lease error does not preserve HTTP status %d: %T %v", status, err, err)
		}
	}

	assignment, err := parseAssignment(fixture.Assignment)
	if err != nil {
		t.Fatal(err)
	}
	if assignment.StepID != "step-fixture" || assignment.LeaseGeneration != 3 || assignment.FencingToken != 9 {
		t.Fatalf("assignment fixture drift: %#v", assignment)
	}

	expectedNonRetry := []string{
		"submit-task-without-idempotency-key",
		"submit-run-without-idempotency-key",
		"signal-run",
		"worker-poll",
	}
	if !reflect.DeepEqual(fixture.NeverRetryWithoutIdentity, expectedNonRetry) {
		t.Fatalf("ambiguous-operation retry contract changed: %#v", fixture.NeverRetryWithoutIdentity)
	}
	if fixture.ProgressChunkID != "{stepId}:{leaseGeneration}:{sequence}" {
		t.Fatalf("progress identity changed: %q", fixture.ProgressChunkID)
	}
}
