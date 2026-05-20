#!/usr/bin/env python3
import base64
import hashlib
import json
import os
import sys
import urllib.request


MAX_COMPILED_FUNCTIONS = int(os.environ.get("LAMBDA_FUNCTION_CACHE_MAX", "128"))
MAX_FUNCTION_BODY_BYTES = int(os.environ.get("LAMBDA_FUNCTION_BODY_MAX_BYTES", "262144"))
MAX_INPUT_LINE_BYTES = int(os.environ.get("LAMBDA_CHILD_INPUT_MAX_BYTES", "6291456"))
MAX_RESULT_BYTES = int(os.environ.get("LAMBDA_RESULT_MAX_BYTES", "1048576"))

compiled_functions = {}
compiled_order = []


class LambdaConsole:
    def _write(self, level, *args):
        rendered = " ".join(str(arg) for arg in args)
        sys.stderr.write(f"[lambda:{level}] {rendered}\n")
        sys.stderr.flush()

    def debug(self, *args):
        self._write("debug", *args)

    def error(self, *args):
        self._write("error", *args)

    def info(self, *args):
        self._write("info", *args)

    def log(self, *args):
        self._write("log", *args)

    def warn(self, *args):
        self._write("warn", *args)


console = LambdaConsole()


def fetch(url, *, method="GET", headers=None, body=None, timeout=10):
    payload = None
    if body is not None:
        if isinstance(body, (dict, list)):
            payload = json.dumps(body).encode("utf-8")
            headers = {"content-type": "application/json", **(headers or {})}
        elif isinstance(body, str):
            payload = body.encode("utf-8")
        else:
            payload = bytes(body)
    request = urllib.request.Request(url, data=payload, headers=headers or {}, method=method)
    with urllib.request.urlopen(request, timeout=timeout) as response:
        raw = response.read(MAX_RESULT_BYTES + 1)
        text = raw.decode("utf-8", errors="replace")
        try:
            parsed = json.loads(text)
        except Exception:
            parsed = None
        return {
            "status": response.status,
            "headers": dict(response.headers.items()),
            "body": parsed if parsed is not None else text,
        }


SAFE_BUILTINS = {
    "abs": abs,
    "all": all,
    "any": any,
    "bool": bool,
    "dict": dict,
    "enumerate": enumerate,
    "float": float,
    "int": int,
    "len": len,
    "list": list,
    "map": map,
    "max": max,
    "min": min,
    "range": range,
    "round": round,
    "set": set,
    "sorted": sorted,
    "str": str,
    "sum": sum,
    "tuple": tuple,
    "zip": zip,
}


def hash_body(body):
    return hashlib.sha256(body.encode("utf-8")).hexdigest()


def compile_function(function_body):
    cache_key = hash_body(function_body)
    cached = compiled_functions.get(cache_key)
    if cached is not None:
        return cached

    try:
        compiled = ("eval", compile(function_body, "<lambda>", "eval"))
    except SyntaxError:
        compiled = ("exec", compile(function_body, "<lambda>", "exec"))

    compiled_functions[cache_key] = compiled
    compiled_order.append(cache_key)
    while len(compiled_order) > MAX_COMPILED_FUNCTIONS:
        oldest = compiled_order.pop(0)
        compiled_functions.pop(oldest, None)
    return compiled


def resolve_definition(envelope):
    definition = envelope.get("definition") or envelope
    if not isinstance(definition, dict) or not definition.get("functionBody"):
        raise ValueError("lambda definition with functionBody is required")
    status = definition.get("status")
    if status in {"paused", "archived"}:
        raise ValueError(f"lambda function is {status}")
    return definition


def invoke(line):
    envelope = json.loads(line)
    definition = resolve_definition(envelope)
    function_body = str(definition.get("functionBody") or "")
    if not function_body.strip():
        raise ValueError("functionBody is required")
    if len(function_body.encode("utf-8")) > MAX_FUNCTION_BODY_BYTES:
        raise ValueError("functionBody exceeds configured byte limit")

    mode, compiled = compile_function(function_body)
    if envelope.get("checkOnly") is True or envelope.get("mode") == "check":
        return {
            "ok": True,
            "check": {
                "runtime": definition.get("runtime"),
                "slug": definition.get("slug") or envelope.get("slug"),
                "mode": mode,
            },
            "cachedFunctions": len(compiled_functions),
        }

    request = envelope.get("request") or {}
    context = {
        "id": definition.get("id"),
        "invocationId": envelope.get("invocationId"),
        "slug": definition.get("slug") or envelope.get("slug"),
        "meta": {
            "runtime": definition.get("runtime"),
            "labels": definition.get("labels"),
            "metaData": definition.get("metaData"),
            **(envelope.get("meta") or {}),
        },
    }
    globals_scope = {
        "__builtins__": SAFE_BUILTINS,
        "base64": base64,
        "console": console,
        "fetch": fetch,
        "json": json,
    }
    locals_scope = {
        "request": request,
        "context": context,
        "console": console,
        "fetch": fetch,
    }

    if mode == "eval":
        result = eval(compiled, globals_scope, locals_scope)
    else:
        exec(compiled, globals_scope, locals_scope)
        handler = locals_scope.get("handler")
        if callable(handler):
            result = handler(request, context)
        else:
            result = locals_scope.get("result")

    return {
        "ok": True,
        "result": result,
        "invocationId": context.get("invocationId"),
        "cachedFunctions": len(compiled_functions),
    }


def write_result(result):
    try:
        encoded = json.dumps(result, separators=(",", ":"))
    except TypeError:
        encoded = json.dumps({"ok": True, "result": str(result)})
    if len(encoded.encode("utf-8")) > MAX_RESULT_BYTES:
        encoded = json.dumps({"ok": False, "error": "lambda result exceeds configured byte limit"})
    sys.stdout.write(encoded + "\n")
    sys.stdout.flush()


def main():
    for line in sys.stdin:
        if len(line.encode("utf-8")) > MAX_INPUT_LINE_BYTES:
            write_result({"ok": False, "error": "lambda input exceeds configured byte limit"})
            continue
        line = line.strip()
        if not line:
            continue
        try:
            write_result(invoke(line))
        except Exception as error:
            # The Erlang parent merges stderr into stdout and treats the first
            # newline-delimited record as the response. Keep handled errors in
            # the JSON response line so diagnostics cannot corrupt the protocol.
            write_result({"ok": False, "error": str(error)})


if __name__ == "__main__":
    main()
