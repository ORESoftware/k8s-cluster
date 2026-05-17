package main

import (
	"encoding/json"
	"fmt"
	"io"
	"os"
)

func main() {
	body, _ := io.ReadAll(os.Stdin)
	response := map[string]any{
		"ok":            true,
		"runtime":       "golang",
		"pid":           os.Getpid(),
		"receivedBytes": len(body),
	}
	encoded, _ := json.Marshal(response)
	fmt.Println(string(encoded))
}
