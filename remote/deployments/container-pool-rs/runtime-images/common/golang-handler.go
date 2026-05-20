package main

import (
	"encoding/json"
	"fmt"
	"io"
	"os"
	"regexp"
	"strconv"
	"strings"
)

func main() {
	body, _ := io.ReadAll(os.Stdin)
	response := map[string]any{
		"ok":            true,
		"runtime":       "golang",
		"pid":           os.Getpid(),
		"receivedBytes": len(body),
	}
	if expr, ok := expressionFromBody(body); ok {
		response["expr"] = expr
		if answer, err := evaluateExpression(expr); err == nil {
			response["answer"] = answer
		} else {
			response["error"] = err.Error()
		}
	}
	encoded, _ := json.Marshal(response)
	fmt.Println(string(encoded))
}

func expressionFromBody(body []byte) (string, bool) {
	var payload any
	if err := json.Unmarshal(body, &payload); err != nil {
		return "", false
	}
	return findExpression(payload)
}

func findExpression(value any) (string, bool) {
	switch typed := value.(type) {
	case map[string]any:
		for _, key := range []string{"expr", "expression"} {
			if raw, ok := typed[key].(string); ok && strings.TrimSpace(raw) != "" {
				return raw, true
			}
		}
		for _, key := range []string{"body", "payload", "request"} {
			if nested, ok := typed[key]; ok {
				if expr, found := findExpression(nested); found {
					return expr, true
				}
			}
		}
	case []any:
		for _, item := range typed {
			if expr, found := findExpression(item); found {
				return expr, true
			}
		}
	}
	return "", false
}

var simpleExpression = regexp.MustCompile(`^\s*(-?\d+)\s*([+\-*/])\s*(-?\d+)\s*$`)

func evaluateExpression(expr string) (int64, error) {
	match := simpleExpression.FindStringSubmatch(expr)
	if match == nil {
		return 0, fmt.Errorf("unsupported expression")
	}
	left, err := strconv.ParseInt(match[1], 10, 64)
	if err != nil {
		return 0, err
	}
	right, err := strconv.ParseInt(match[3], 10, 64)
	if err != nil {
		return 0, err
	}
	switch match[2] {
	case "+":
		return left + right, nil
	case "-":
		return left - right, nil
	case "*":
		return left * right, nil
	case "/":
		if right == 0 {
			return 0, fmt.Errorf("division by zero")
		}
		return left / right, nil
	default:
		return 0, fmt.Errorf("unsupported operator")
	}
}
