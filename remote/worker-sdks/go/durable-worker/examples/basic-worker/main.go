package main

import (
	"context"
	"fmt"
	"log"
	"os"
	"os/signal"
	"syscall"

	durableworker "github.com/oresoftware/k8s-cluster/remote/worker-sdks/go/durable-worker"
)

func main() {
	client, err := durableworker.NewClient(
		os.Getenv("DURABLE_WORKER_URL"),
		os.Getenv("DURABLE_WORKER_AUTH_SECRET"),
		durableworker.ClientOptions{},
	)
	if err != nil {
		log.Fatal(err)
	}

	ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGINT, syscall.SIGTERM)
	defer stop()

	summary, err := durableworker.RunWorker(ctx, client, map[string]durableworker.Handler{
		"example:double": func(task *durableworker.TaskContext) (durableworker.JSON, error) {
			value, _ := task.Input()["value"].(float64)
			if _, err := task.Emit("starting", durableworker.OutputOptions{}); err != nil {
				return nil, err
			}

			// A real downstream effect should be keyed by the logical operation
			// and protected by task.FencingToken() when the destination supports it.
			effectIdentity := fmt.Sprintf("%s:%d", task.StepID(), task.FencingToken())
			log.Printf("effect identity: %s", effectIdentity)
			return durableworker.JSON{"value": value * 2}, nil
		},
	}, durableworker.WorkerConfig{
		WorkerID:     hostname("durable-go-worker"),
		Queues:       []string{"default"},
		Capabilities: []string{"example:double"},
		Slots:        4,
	})
	if err != nil {
		log.Fatal(err)
	}
	log.Printf("worker stopped: %+v", summary)
}

func hostname(fallback string) string {
	value, err := os.Hostname()
	if err != nil || value == "" {
		return fallback
	}
	return value
}
