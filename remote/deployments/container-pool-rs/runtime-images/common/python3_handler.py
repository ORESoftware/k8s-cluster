#!/usr/bin/env python3
import json
import os
import sys

raw = sys.stdin.read()
try:
    request = json.loads(raw) if raw else {}
except json.JSONDecodeError as error:
    request = {"parseError": str(error), "raw": raw}

print(json.dumps({
    "ok": True,
    "runtime": "python3",
    "pid": os.getpid(),
    "request": request,
}, separators=(",", ":")))
