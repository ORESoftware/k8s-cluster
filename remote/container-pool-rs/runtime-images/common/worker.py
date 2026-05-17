#!/usr/bin/env python3
import json
import os
import shlex
import subprocess
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


MAX_BODY_BYTES = int(os.getenv("DD_POOL_MAX_BODY_BYTES", str(2 * 1024 * 1024)))
REQUEST_TIMEOUT_SECONDS = float(os.getenv("DD_POOL_HANDLER_TIMEOUT_SECONDS", "30"))
RUNTIME = os.getenv("DD_POOL_RUNTIME", "generic")
HANDLER = os.getenv("DD_POOL_HANDLER", "")


def json_response(handler, status, payload):
    body = json.dumps(payload, separators=(",", ":")).encode("utf-8")
    handler.send_response(status)
    handler.send_header("Content-Type", "application/json")
    handler.send_header("Content-Length", str(len(body)))
    handler.end_headers()
    handler.wfile.write(body)


def parse_handler_output(stdout):
    text = stdout.decode("utf-8", errors="replace")
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        return {"text": text}


def parse_request_body(body):
    try:
        return json.loads(body or b"{}")
    except json.JSONDecodeError:
        return {"raw": body.decode("utf-8", errors="replace")}


class PoolWorker(BaseHTTPRequestHandler):
    server_version = "dd-container-pool-worker/0.1"

    def do_GET(self):
        if self.path != "/healthz":
            json_response(self, 404, {"ok": False, "error": "not found"})
            return
        json_response(self, 200, {"ok": True, "runtime": RUNTIME, "handlerConfigured": bool(HANDLER)})

    def do_POST(self):
        if self.path != "/invoke":
            json_response(self, 404, {"ok": False, "error": "not found"})
            return
        length = int(self.headers.get("content-length", "0") or "0")
        if length > MAX_BODY_BYTES:
            json_response(self, 413, {"ok": False, "error": "request body too large"})
            return
        body = self.rfile.read(length)
        request_payload = parse_request_body(body)
        echo_key = request_payload.get("echoKey") or request_payload.get("key")
        if not HANDLER:
            json_response(
                self,
                200,
                {"ok": True, "runtime": RUNTIME, "echoKey": echo_key, "request": request_payload},
            )
            return

        command = shlex.split(HANDLER)
        try:
            completed = subprocess.run(
                command,
                input=body,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=REQUEST_TIMEOUT_SECONDS,
                check=False,
            )
        except subprocess.TimeoutExpired:
            json_response(self, 504, {"ok": False, "runtime": RUNTIME, "error": "handler timed out"})
            return
        except OSError as error:
            json_response(self, 502, {"ok": False, "runtime": RUNTIME, "error": str(error)})
            return

        payload = {
            "ok": completed.returncode == 0,
            "runtime": RUNTIME,
            "echoKey": echo_key,
            "request": request_payload,
            "handlerStatus": completed.returncode,
            "result": parse_handler_output(completed.stdout),
        }
        if completed.stderr:
            payload["stderr"] = completed.stderr.decode("utf-8", errors="replace")[:4096]
        json_response(self, 200 if completed.returncode == 0 else 502, payload)

    def log_message(self, fmt, *args):
        print("%s - %s" % (self.address_string(), fmt % args), flush=True)


def main():
    port = int(os.getenv("PORT", "8080"))
    server = ThreadingHTTPServer(("0.0.0.0", port), PoolWorker)
    print(f"dd container pool worker listening on :{port} runtime={RUNTIME}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
