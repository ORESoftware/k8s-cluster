#!/usr/bin/env python3
import json
import os
import shlex
import socket
import subprocess
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse


MAX_BODY_BYTES = int(os.getenv("DD_POOL_MAX_BODY_BYTES", str(2 * 1024 * 1024)))
REQUEST_TIMEOUT_SECONDS = float(os.getenv("DD_POOL_HANDLER_TIMEOUT_SECONDS", "30"))
RUNTIME = os.getenv("DD_POOL_RUNTIME", "generic")
HANDLER = os.getenv("DD_POOL_HANDLER", "")
REQUEST_PATH = os.getenv("DD_POOL_REQUEST_PATH", "/invoke")
HEALTH_PATH = os.getenv("DD_POOL_HEALTH_PATH", "/healthz")
POOL_ID = os.getenv("DD_POOL_ID", "")
POOL_SLUG = os.getenv("DD_POOL_SLUG", RUNTIME)
CONTAINER_NAME = os.getenv("DD_POOL_CONTAINER_NAME", "")
NATS_URL = os.getenv("NATS_URL", "")
NATS_EVENT_SUBJECT = os.getenv("DD_POOL_NATS_EVENT_SUBJECT", "")
NATS_HEARTBEAT_SUBJECT = os.getenv("DD_POOL_NATS_HEARTBEAT_SUBJECT", "")
NATS_HEARTBEAT_SECONDS = float(os.getenv("DD_POOL_NATS_HEARTBEAT_SECONDS", "15"))


def nats_connect_payload(parsed):
    payload = {
        "verbose": False,
        "pedantic": False,
        "lang": "python",
        "name": CONTAINER_NAME or POOL_SLUG,
    }
    if parsed.username and parsed.password:
        payload["user"] = parsed.username
        payload["pass"] = parsed.password
    elif parsed.username:
        payload["auth_token"] = parsed.username
    return json.dumps(payload, separators=(",", ":")).encode("utf-8")


def publish_nats(subject, payload):
    if not NATS_URL or not subject:
        return
    parsed = urlparse(NATS_URL)
    if parsed.scheme != "nats" or not parsed.hostname:
        return
    body = json.dumps(payload, separators=(",", ":")).encode("utf-8")
    with socket.create_connection((parsed.hostname, parsed.port or 4222), timeout=2) as sock:
        sock.settimeout(2)
        try:
            sock.recv(4096)
        except OSError:
            pass
        connect = nats_connect_payload(parsed)
        sock.sendall(b"CONNECT " + connect + b"\r\n")
        sock.sendall(
            b"PUB "
            + subject.encode("utf-8")
            + b" "
            + str(len(body)).encode("ascii")
            + b"\r\n"
        )
        sock.sendall(body + b"\r\nPING\r\n")


def event_payload(event, extra=None):
    payload = {
        "event": event,
        "runtime": RUNTIME,
        "poolId": POOL_ID,
        "poolSlug": POOL_SLUG,
        "containerName": CONTAINER_NAME,
        "generatedAtMs": int(time.time() * 1000),
    }
    if extra:
        payload.update(extra)
    return payload


def emit_event(event, extra=None, subject=None):
    destination = subject or NATS_EVENT_SUBJECT
    if not destination:
        return
    thread = threading.Thread(
        target=lambda: publish_nats(destination, event_payload(event, extra)),
        daemon=True,
    )
    thread.start()


def heartbeat_loop():
    while True:
        emit_event("heartbeat", subject=NATS_HEARTBEAT_SUBJECT)
        time.sleep(max(1.0, NATS_HEARTBEAT_SECONDS))


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
        path = self.path.split("?", 1)[0]
        if path != HEALTH_PATH:
            json_response(self, 404, {"ok": False, "error": "not found"})
            return
        json_response(
            self,
            200,
            {
                "ok": True,
                "runtime": RUNTIME,
                "poolSlug": POOL_SLUG,
                "containerName": CONTAINER_NAME,
                "handlerConfigured": bool(HANDLER),
                "natsEventsConfigured": bool(NATS_EVENT_SUBJECT),
                "natsHeartbeatsConfigured": bool(NATS_HEARTBEAT_SUBJECT),
            },
        )

    def do_POST(self):
        path = self.path.split("?", 1)[0]
        if path != REQUEST_PATH:
            json_response(self, 404, {"ok": False, "error": "not found"})
            return
        length = int(self.headers.get("content-length", "0") or "0")
        if length > MAX_BODY_BYTES:
            json_response(self, 413, {"ok": False, "error": "request body too large"})
            return
        body = self.rfile.read(length)
        request_payload = parse_request_body(body)
        echo_key = request_payload.get("echoKey") or request_payload.get("key")
        emit_event("request.received", {"echoKey": echo_key})
        if not HANDLER:
            emit_event("request.completed", {"ok": True, "status": 200, "echoKey": echo_key})
            json_response(
                self,
                200,
                {
                    "ok": True,
                    "runtime": RUNTIME,
                    "echoKey": echo_key,
                    "request": request_payload,
                },
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
            emit_event("request.completed", {"ok": False, "status": 504, "echoKey": echo_key})
            json_response(self, 504, {"ok": False, "runtime": RUNTIME, "error": "handler timed out"})
            return
        except OSError as error:
            emit_event("request.completed", {"ok": False, "status": 502, "echoKey": echo_key})
            json_response(self, 502, {"ok": False, "runtime": RUNTIME, "error": str(error)})
            return

        status = 200 if completed.returncode == 0 else 502
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
        emit_event(
            "request.completed",
            {"ok": completed.returncode == 0, "status": status, "echoKey": echo_key},
        )
        json_response(self, status, payload)

    def log_message(self, fmt, *args):
        print("%s - %s" % (self.address_string(), fmt % args), flush=True)


def main():
    port = int(os.getenv("PORT", "8080"))
    emit_event("started")
    if NATS_HEARTBEAT_SUBJECT:
        threading.Thread(target=heartbeat_loop, daemon=True).start()
    server = ThreadingHTTPServer(("0.0.0.0", port), PoolWorker)
    print(
        "dd container pool worker listening on "
        f":{port} runtime={RUNTIME} requestPath={REQUEST_PATH} healthPath={HEALTH_PATH}",
        flush=True,
    )
    server.serve_forever()


if __name__ == "__main__":
    main()
