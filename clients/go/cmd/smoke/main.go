// End-to-end smoke test mirroring clients/ts/src/smoke.ts.
//
//   go run ./cmd/smoke
//
// Override host/port via LIVE_MUTEX_HOST / LIVE_MUTEX_PORT.
package main

import (
	"context"
	"fmt"
	"log"
	"os"
	"sync"
	"time"

	networkmutex "github.com/oresoftware/dd/rust-network-mutex/clients/go"
)

func envOr(key, def string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return def
}

func main() {
	host := envOr("LIVE_MUTEX_HOST", "127.0.0.1")
	port := envOr("LIVE_MUTEX_PORT", "6970")
	addr := fmt.Sprintf("%s:%s", host, port)

	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	client, err := networkmutex.Dial(ctx, networkmutex.Options{Address: addr})
	if err != nil {
		log.Fatalf("[smoke-go] dial: %v", err)
	}
	defer client.Close()
	log.Printf("[smoke-go] connected %s", addr)

	exHandle, err := client.Acquire(ctx, "smoke-go-exclusive", 5*time.Second)
	if err != nil {
		log.Fatalf("[smoke-go] acquire: %v", err)
	}
	log.Printf("[smoke-go] exclusive grant: lockUuid=%s fencing=%d", exHandle.LockUUID, exHandle.FencingToken)
	if err := client.Release(ctx, exHandle); err != nil {
		log.Fatalf("[smoke-go] release: %v", err)
	}
	log.Printf("[smoke-go] released exclusive")

	comp, err := client.AcquireMany(ctx, []string{"smoke-go-a", "smoke-go-b", "smoke-go-c"}, 5*time.Second)
	if err != nil {
		log.Fatalf("[smoke-go] acquireMany: %v", err)
	}
	log.Printf("[smoke-go] composite grant: lockUuid=%s tokens=%v", comp.LockUUID, comp.FencingTokens)
	if err := client.Release(ctx, comp); err != nil {
		log.Fatalf("[smoke-go] release composite: %v", err)
	}
	log.Printf("[smoke-go] released composite")

	wid, wt, err := client.AcquireWrite(ctx, "smoke-go-rw")
	if err != nil {
		log.Fatalf("[smoke-go] acquire write: %v", err)
	}
	log.Printf("[smoke-go] writer grant: id=%s fencing=%d", wid, wt)
	if err := client.ReleaseWrite(ctx, "smoke-go-rw"); err != nil {
		log.Fatalf("[smoke-go] release write: %v", err)
	}

	var wg sync.WaitGroup
	wg.Add(2)
	for i := 0; i < 2; i++ {
		go func(idx int) {
			defer wg.Done()
			id, t, err := client.AcquireRead(ctx, "smoke-go-rw")
			if err != nil {
				log.Fatalf("[smoke-go] acquire read %d: %v", idx, err)
			}
			log.Printf("[smoke-go] reader %d grant: id=%s fencing=%d", idx, id, t)
		}(i)
	}
	wg.Wait()
	for i := 0; i < 2; i++ {
		if err := client.ReleaseRead(ctx, "smoke-go-rw"); err != nil {
			log.Fatalf("[smoke-go] release read: %v", err)
		}
	}

	log.Printf("[smoke-go] OK")
}
