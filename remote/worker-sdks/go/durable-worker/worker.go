package durableworker

import (
	"context"
	"errors"
	"fmt"
	"runtime/debug"
	"strconv"
	"sync"
	"sync/atomic"
	"time"
)

// WorkerClient is the subset of the protocol used by RunWorker. *Client
// implements this interface; tests and adapters may provide their own client.
type WorkerClient interface {
	RegisterWorker(context.Context, JSON) (JSON, error)
	HeartbeatWorker(context.Context, string, *bool) (JSON, error)
	PollWorker(context.Context, string, time.Duration) (JSON, error)
	StartStep(context.Context, string, Lease) (JSON, error)
	HeartbeatStep(context.Context, string, Lease) (JSON, error)
	AppendStepOutput(context.Context, string, Lease, string, string, string, bool) (JSON, error)
	CompleteStep(context.Context, string, Lease, JSON) (JSON, error)
	FailStep(context.Context, string, Lease, string, string, bool) (JSON, error)
}

// WorkerConfig controls worker registration, admission, and heartbeat cadence.
type WorkerConfig struct {
	WorkerID        string
	Queues          []string
	Capabilities    []string
	Labels          JSON
	Slots           int
	TTL             time.Duration
	PollWait        time.Duration
	WorkerHeartbeat time.Duration
	StepHeartbeat   time.Duration
	IdleSleep       time.Duration
	// MaxAssignments is zero for an unbounded worker and positive for a
	// deterministic bounded worker (useful for serverless and tests).
	MaxAssignments int
}

func (config WorkerConfig) withDefaults() WorkerConfig {
	if config.Slots == 0 {
		config.Slots = 1
	}
	if config.TTL == 0 {
		config.TTL = 45 * time.Second
	}
	if config.PollWait == 0 {
		config.PollWait = 30 * time.Second
	}
	if config.WorkerHeartbeat == 0 {
		config.WorkerHeartbeat = 15 * time.Second
	}
	if config.StepHeartbeat == 0 {
		config.StepHeartbeat = 15 * time.Second
	}
	if config.IdleSleep == 0 {
		config.IdleSleep = 100 * time.Millisecond
	}
	if config.Labels == nil {
		config.Labels = JSON{}
	}
	return config
}

func (config WorkerConfig) validate() error {
	if config.WorkerID == "" {
		return errors.New("worker ID must be non-empty")
	}
	if len(config.Queues) == 0 {
		return errors.New("at least one queue is required")
	}
	if config.Slots <= 0 {
		return errors.New("slots must be positive")
	}
	if config.TTL <= 0 || config.WorkerHeartbeat <= 0 || config.StepHeartbeat <= 0 {
		return errors.New("TTL and heartbeat intervals must be positive")
	}
	if config.PollWait < 0 || config.IdleSleep < 0 || config.MaxAssignments < 0 {
		return errors.New("poll wait, idle sleep, and max assignments must be non-negative")
	}
	return nil
}

// WorkerSummary separates protocol errors from server-acknowledged task
// failures so operators do not mistake an ambiguous terminal write for a
// durable failed state.
type WorkerSummary struct {
	Accepted       int
	Completed      int
	Failed         int
	LeaseLost      int
	ProtocolErrors int
}

type mutableSummary struct {
	mu      sync.Mutex
	summary WorkerSummary
}

func (summary *mutableSummary) accepted() int {
	summary.mu.Lock()
	defer summary.mu.Unlock()
	return summary.summary.Accepted
}

func (summary *mutableSummary) snapshot() WorkerSummary {
	summary.mu.Lock()
	defer summary.mu.Unlock()
	return summary.summary
}

// Assignment is one leased task delivery.
type Assignment struct {
	RunID            string
	StepID           string
	StepKey          string
	TaskType         string
	Queue            string
	Input            JSON
	Attempt          int64
	LeaseToken       string
	LeaseGeneration  int64
	FencingToken     int64
	LeaseExpiresAtMS int64
	TimeoutMS        int64
	AffinityKey      any
	Raw              JSON
}

func parseAssignment(raw JSON) (Assignment, error) {
	assignment := Assignment{
		RunID:            stringValue(raw["runId"]),
		StepID:           stringValue(raw["stepId"]),
		StepKey:          stringValue(raw["stepKey"]),
		TaskType:         stringValue(raw["taskType"]),
		Queue:            stringValue(raw["queue"]),
		Attempt:          intValue(raw["attempt"]),
		LeaseToken:       stringValue(raw["leaseToken"]),
		LeaseGeneration:  intValue(raw["leaseGeneration"]),
		FencingToken:     intValue(raw["fencingToken"]),
		LeaseExpiresAtMS: intValue(raw["leaseExpiresAtMs"]),
		TimeoutMS:        intValue(raw["timeoutMs"]),
		AffinityKey:      raw["affinityKey"],
		Raw:              cloneJSON(raw),
	}
	if input, ok := objectValue(raw["input"]); ok {
		assignment.Input = input
	} else {
		assignment.Input = JSON{}
	}
	if assignment.RunID == "" || assignment.StepID == "" || assignment.TaskType == "" || assignment.LeaseToken == "" {
		return Assignment{}, errors.New("assignment is missing runId, stepId, taskType, or leaseToken")
	}
	if assignment.LeaseGeneration < 1 || assignment.FencingToken < 1 {
		return Assignment{}, errors.New("assignment lease generation and fencing token must be positive")
	}
	return assignment, nil
}

func intValue(value any) int64 {
	switch typed := value.(type) {
	case int:
		return int64(typed)
	case int64:
		return typed
	case float64:
		return int64(typed)
	case jsonNumber:
		parsed, _ := strconv.ParseInt(string(typed), 10, 64)
		return parsed
	case string:
		parsed, _ := strconv.ParseInt(typed, 10, 64)
		return parsed
	default:
		return 0
	}
}

// jsonNumber is intentionally local: the stdlib JSON decoder currently emits
// float64 into map[string]any, while adapters may preserve number text.
type jsonNumber string

func cloneJSON(source map[string]any) JSON {
	result := make(JSON, len(source))
	for key, value := range source {
		result[key] = value
	}
	return result
}

func objectValue(value any) (JSON, bool) {
	switch typed := value.(type) {
	case JSON:
		return cloneJSON(typed), true
	case map[string]any:
		return cloneJSON(typed), true
	default:
		return nil, false
	}
}

// WorkerFailure lets a handler choose the durable failure code and retryability.
type WorkerFailure struct {
	Code      string
	Message   string
	Retryable bool
}

func (failure *WorkerFailure) Error() string {
	if failure == nil {
		return "worker failure"
	}
	return failure.Message
}

// Handler executes one assignment. It must observe TaskContext.Context and
// make downstream effects idempotent or fencing-token aware.
type Handler func(*TaskContext) (JSON, error)

// OutputOptions controls one persisted output chunk.
type OutputOptions struct {
	ChunkID string
	Stream  string
	Final   bool
}

// TaskContext exposes the assignment, cancellation, progress, and fencing token.
type TaskContext struct {
	ctx        context.Context
	cancel     context.CancelCauseFunc
	client     WorkerClient
	assignment Assignment
	lease      Lease
	outputMu   sync.Mutex
	sequence   uint64
}

func (task *TaskContext) Context() context.Context { return task.ctx }
func (task *TaskContext) Assignment() Assignment   { return task.assignment }
func (task *TaskContext) Input() JSON              { return cloneJSON(task.assignment.Input) }
func (task *TaskContext) RunID() string            { return task.assignment.RunID }
func (task *TaskContext) StepID() string           { return task.assignment.StepID }
func (task *TaskContext) FencingToken() int64      { return task.assignment.FencingToken }

// RaiseIfCancelled turns lease uncertainty into an explicit error.
func (task *TaskContext) RaiseIfCancelled() error {
	select {
	case <-task.ctx.Done():
		cause := context.Cause(task.ctx)
		if cause != nil {
			return cause
		}
		return task.ctx.Err()
	default:
		return nil
	}
}

// Emit appends an idempotent output chunk. Generated chunk IDs are scoped to
// step ID and lease generation so redelivery cannot collide with a stale worker.
func (task *TaskContext) Emit(chunk string, options OutputOptions) (JSON, error) {
	if err := task.RaiseIfCancelled(); err != nil {
		return nil, err
	}
	task.outputMu.Lock()
	task.sequence++
	chunkID := options.ChunkID
	if chunkID == "" {
		chunkID = fmt.Sprintf("%s:%d:%d", task.assignment.StepID, task.lease.LeaseGeneration, task.sequence)
	}
	task.outputMu.Unlock()
	stream := options.Stream
	if stream == "" {
		stream = "progress"
	}
	result, err := task.client.AppendStepOutput(task.ctx, task.assignment.StepID, task.lease, chunkID, chunk, stream, options.Final)
	if IsLeaseLost(err) {
		task.cancel(err)
	}
	return result, err
}

// RunWorker registers a worker, long-polls only when a local slot is available,
// renews worker and step leases, drains accepted work on shutdown, and rejects
// stale terminal writes after fencing.
func RunWorker(ctx context.Context, client WorkerClient, handlers map[string]Handler, rawConfig WorkerConfig) (WorkerSummary, error) {
	config := rawConfig.withDefaults()
	if err := config.validate(); err != nil {
		return WorkerSummary{}, err
	}

	registration := JSON{
		"workerId":     config.WorkerID,
		"queues":       append([]string(nil), config.Queues...),
		"capabilities": append([]string(nil), config.Capabilities...),
		"labels":       cloneJSON(config.Labels),
		"slots":        config.Slots,
		"ttlMs":        config.TTL.Milliseconds(),
		"drain":        false,
	}
	if _, err := client.RegisterWorker(ctx, registration); err != nil {
		return WorkerSummary{}, err
	}

	pollContext, cancelPoll := context.WithCancel(ctx)
	defer cancelPoll()

	var draining atomic.Bool
	heartbeatStop := make(chan struct{})
	heartbeatDone := make(chan struct{})
	fatalHeartbeat := make(chan error, 1)
	go workerHeartbeatLoop(client, config, &draining, cancelPoll, heartbeatStop, heartbeatDone, fatalHeartbeat)

	summary := &mutableSummary{}
	semaphore := make(chan struct{}, config.Slots)
	var tasks sync.WaitGroup
	var loopErr error

acceptLoop:
	for {
		if config.MaxAssignments > 0 && summary.accepted() >= config.MaxAssignments {
			break
		}
		select {
		case <-ctx.Done():
			break acceptLoop
		case err := <-fatalHeartbeat:
			loopErr = err
			break acceptLoop
		case semaphore <- struct{}{}:
		}

		poll, err := client.PollWorker(pollContext, config.WorkerID, config.PollWait)
		if err != nil {
			<-semaphore
			if ctx.Err() != nil {
				break
			}
			select {
			case heartbeatErr := <-fatalHeartbeat:
				loopErr = heartbeatErr
			default:
				loopErr = err
			}
			break
		}
		rawAssignment, exists := poll["assignment"]
		if !exists || rawAssignment == nil {
			<-semaphore
			retryAfter := config.IdleSleep
			if milliseconds := intValue(poll["retryAfterMs"]); milliseconds >= 0 && milliseconds != 0 {
				retryAfter = time.Duration(milliseconds) * time.Millisecond
			}
			if retryAfter > 0 {
				select {
				case <-ctx.Done():
					break acceptLoop
				case <-time.After(retryAfter):
				}
			}
			continue
		}
		assignmentMap, ok := objectValue(rawAssignment)
		if !ok {
			<-semaphore
			loopErr = &Error{Code: "invalid_assignment", Message: "poll response contained a non-object assignment"}
			break
		}
		assignment, err := parseAssignment(assignmentMap)
		if err != nil {
			<-semaphore
			loopErr = &Error{Code: "invalid_assignment", Message: err.Error(), Cause: err}
			break
		}

		summary.mu.Lock()
		summary.summary.Accepted++
		summary.mu.Unlock()
		tasks.Add(1)
		go func() {
			defer tasks.Done()
			defer func() { <-semaphore }()
			executeAssignment(client, handlers, config, assignment, summary)
		}()
	}

	draining.Store(true)
	cancelPoll()
	tasks.Wait()
	close(heartbeatStop)
	<-heartbeatDone
	finalDrain := true
	finalContext, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	_, _ = client.HeartbeatWorker(finalContext, config.WorkerID, &finalDrain)
	cancel()

	return summary.snapshot(), loopErr
}

func workerHeartbeatLoop(
	client WorkerClient,
	config WorkerConfig,
	draining *atomic.Bool,
	cancelPoll context.CancelFunc,
	stop <-chan struct{},
	done chan<- struct{},
	fatal chan<- error,
) {
	defer close(done)
	ticker := time.NewTicker(config.WorkerHeartbeat)
	defer ticker.Stop()
	for {
		select {
		case <-stop:
			return
		case <-ticker.C:
			drain := draining.Load()
			ctx, cancel := context.WithTimeout(context.Background(), minDuration(config.WorkerHeartbeat, 10*time.Second))
			_, err := client.HeartbeatWorker(ctx, config.WorkerID, &drain)
			cancel()
			if err == nil {
				continue
			}
			var protocolErr *Error
			if errors.As(err, &protocolErr) && protocolErr.Retryable {
				continue
			}
			select {
			case fatal <- err:
			default:
			}
			cancelPoll()
			return
		}
	}
}

func executeAssignment(client WorkerClient, handlers map[string]Handler, config WorkerConfig, assignment Assignment, summary *mutableSummary) {
	lease := Lease{WorkerID: config.WorkerID, LeaseToken: assignment.LeaseToken, LeaseGeneration: assignment.LeaseGeneration}
	startContext, cancelStart := context.WithTimeout(context.Background(), 15*time.Second)
	_, err := client.StartStep(startContext, assignment.StepID, lease)
	cancelStart()
	if err != nil {
		summary.mu.Lock()
		if IsLeaseLost(err) {
			summary.summary.LeaseLost++
		} else {
			summary.summary.ProtocolErrors++
		}
		summary.mu.Unlock()
		return
	}

	baseContext := context.Background()
	var timeoutCancel context.CancelFunc
	if assignment.TimeoutMS > 0 {
		baseContext, timeoutCancel = context.WithTimeout(baseContext, time.Duration(assignment.TimeoutMS)*time.Millisecond)
	} else {
		timeoutCancel = func() {}
	}
	defer timeoutCancel()
	taskContext, cancelTask := context.WithCancelCause(baseContext)
	defer cancelTask(nil)
	task := &TaskContext{ctx: taskContext, cancel: cancelTask, client: client, assignment: assignment, lease: lease}

	heartbeatStop := make(chan struct{})
	heartbeatDone := make(chan struct{})
	go stepHeartbeatLoop(client, assignment.StepID, lease, config.StepHeartbeat, cancelTask, heartbeatStop, heartbeatDone)

	handler := handlers[assignment.TaskType]
	result, handlerErr := invokeHandler(handler, task, assignment.TaskType)
	close(heartbeatStop)
	<-heartbeatDone

	if cancellation := task.RaiseIfCancelled(); cancellation != nil {
		summary.mu.Lock()
		if IsLeaseLost(cancellation) {
			summary.summary.LeaseLost++
			summary.mu.Unlock()
			return
		}
		summary.mu.Unlock()
		if errors.Is(cancellation, context.DeadlineExceeded) {
			applyTerminalReport(summary, reportFailure(client, assignment.StepID, lease, "handler_timeout", "handler exceeded assignment timeout", true))
		} else {
			summary.mu.Lock()
			summary.summary.ProtocolErrors++
			summary.mu.Unlock()
		}
		return
	}

	if handlerErr != nil {
		code := "handler_error"
		message := handlerErr.Error()
		retryable := false
		var failure *WorkerFailure
		if errors.As(handlerErr, &failure) {
			code = failure.Code
			message = failure.Message
			retryable = failure.Retryable
		}
		applyTerminalReport(summary, reportFailure(client, assignment.StepID, lease, code, message, retryable))
		return
	}

	completeContext, cancelComplete := context.WithTimeout(context.Background(), 15*time.Second)
	_, err = client.CompleteStep(completeContext, assignment.StepID, lease, result)
	cancelComplete()
	summary.mu.Lock()
	defer summary.mu.Unlock()
	if err == nil {
		summary.summary.Completed++
	} else if IsLeaseLost(err) {
		summary.summary.LeaseLost++
	} else {
		summary.summary.ProtocolErrors++
	}
}

func stepHeartbeatLoop(
	client WorkerClient,
	stepID string,
	lease Lease,
	interval time.Duration,
	cancelTask context.CancelCauseFunc,
	stop <-chan struct{},
	done chan<- struct{},
) {
	defer close(done)
	ticker := time.NewTicker(interval)
	defer ticker.Stop()
	for {
		select {
		case <-stop:
			return
		case <-ticker.C:
			ctx, cancel := context.WithTimeout(context.Background(), minDuration(interval, 10*time.Second))
			_, err := client.HeartbeatStep(ctx, stepID, lease)
			cancel()
			if err != nil {
				if !IsLeaseLost(err) {
					err = &LeaseLostError{Protocol: &Error{
						Code:      "lease_heartbeat_uncertain",
						Message:   "step heartbeat failed; lease authority is uncertain",
						Retryable: true,
						Cause:     err,
					}}
				}
				cancelTask(err)
				return
			}
		}
	}
}

func invokeHandler(handler Handler, task *TaskContext, taskType string) (result JSON, err error) {
	if handler == nil {
		return nil, &WorkerFailure{Code: "handler_not_found", Message: "no handler registered for task type " + taskType, Retryable: false}
	}
	defer func() {
		if recovered := recover(); recovered != nil {
			err = &WorkerFailure{
				Code:      "handler_panic",
				Message:   fmt.Sprintf("handler panicked: %v\n%s", recovered, debug.Stack()),
				Retryable: false,
			}
		}
	}()
	result, err = handler(task)
	if result == nil {
		result = JSON{}
	}
	return result, err
}

type terminalReport int

const (
	terminalReported terminalReport = iota
	terminalFenced
	terminalProtocolError
)

func reportFailure(client WorkerClient, stepID string, lease Lease, code, message string, retryable bool) terminalReport {
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	_, err := client.FailStep(ctx, stepID, lease, code, message, retryable)
	cancel()
	if err == nil {
		return terminalReported
	}
	if IsLeaseLost(err) {
		return terminalFenced
	}
	return terminalProtocolError
}

func applyTerminalReport(summary *mutableSummary, report terminalReport) {
	summary.mu.Lock()
	defer summary.mu.Unlock()
	switch report {
	case terminalReported:
		summary.summary.Failed++
	case terminalFenced:
		summary.summary.LeaseLost++
	case terminalProtocolError:
		summary.summary.ProtocolErrors++
	}
}

func minDuration(left, right time.Duration) time.Duration {
	if left < right {
		return left
	}
	return right
}
