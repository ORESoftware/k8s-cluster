"""Offline protocol round-trip tests (no broker needed).

    python3 -m unittest discover -s clients/python -p 'test_*.py'
"""

import json
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from network_mutex import NetworkMutexClient, protocol  # noqa: E402
from network_mutex.protocol import Response, ResponseType  # noqa: E402


def decode_frame(frame: bytes):
    assert frame.endswith(b"\n"), "frames must be newline-terminated"
    return json.loads(frame[:-1])


class ProtocolTests(unittest.TestCase):
    def test_lock_request_uses_camel_case_wire_fields(self):
        obj = decode_frame(
            protocol.lock_request("u-1", key="k1", ttl_ms=4000, max_holders=1, pid=123)
        )
        self.assertEqual(obj["type"], "lock")
        self.assertEqual(obj["key"], "k1")
        self.assertEqual(obj["ttl"], 4000)
        self.assertEqual(obj["max"], 1)
        self.assertEqual(obj["pid"], 123)
        self.assertNotIn("keys", obj)  # None fields stripped

    def test_composite_lock_request_sorted_by_broker_not_client(self):
        obj = decode_frame(protocol.lock_request("u-2", keys=["c", "a", "b"], wait=False))
        self.assertEqual(obj["type"], "lock")
        self.assertEqual(obj["keys"], ["c", "a", "b"])  # client does not sort
        self.assertFalse(obj["wait"])

    def test_lock_request_preserves_wait_true(self):
        obj = decode_frame(protocol.lock_request("u-3", key="k", wait=True))
        self.assertTrue(obj["wait"])

    def test_lock_request_omits_wait_by_default(self):
        obj = decode_frame(protocol.lock_request("u-4", key="k"))
        self.assertNotIn("wait", obj)

    def test_lock_request_rejects_both_key_and_keys(self):
        with self.assertRaises(ValueError):
            protocol.lock_request("u", key="k", keys=["a"])

    def test_lock_request_rejects_neither_key_nor_keys(self):
        with self.assertRaises(ValueError):
            protocol.lock_request("u")

    def test_lock_request_rejects_oversized_composite(self):
        with self.assertRaises(ValueError):
            protocol.lock_request("u", keys=["a", "b", "c", "d", "e", "f"])

    def test_register_read_request_tag(self):
        obj = decode_frame(protocol.register_read_request("u", "k"))
        self.assertEqual(obj["type"], "registerRead")

    def test_response_parses_composite_grant(self):
        resp = Response.decode(
            json.dumps(
                {
                    "type": "compositeLock",
                    "uuid": "u-1",
                    "keys": ["a", "b"],
                    "acquired": True,
                    "lockUuid": "L-1",
                    "fencingTokens": {"a": 5, "b": 12},
                }
            ).encode()
        )
        self.assertIs(resp.type, ResponseType.COMPOSITE_LOCK)
        self.assertTrue(resp.acquired)
        self.assertEqual(resp.lock_uuid, "L-1")
        self.assertEqual(resp.fencing_tokens, {"a": 5, "b": 12})

    def test_response_distinguishes_absent_from_false(self):
        resp = Response.decode(json.dumps({"type": "ok", "uuid": "u"}).encode())
        self.assertIs(resp.type, ResponseType.OK)
        self.assertIsNone(resp.acquired)  # absent, not False

    def test_unknown_response_type_raises(self):
        with self.assertRaises(ValueError):
            Response.decode(json.dumps({"type": "totallyBogus", "uuid": "u"}).encode())


class StubClient(NetworkMutexClient):
    def __init__(self, responses):
        self.frames = []
        self.responses = list(responses)

    def _roundtrip(self, frame: bytes, uuid: str, *, timeout=None) -> Response:
        self.frames.append(decode_frame(frame))
        return self.responses.pop(0)

    def _roundtrip_grant(self, frame: bytes, uuid: str, *, timeout=None) -> Response:
        self.frames.append(decode_frame(frame))
        return self.responses.pop(0)


class ClientWaitTests(unittest.TestCase):
    def test_try_acquire_many_sends_wait_false_and_returns_none(self):
        client = StubClient(
            [
                Response.from_dict(
                    {
                        "type": "compositeLock",
                        "uuid": "u",
                        "keys": ["a", "b"],
                        "acquired": False,
                    }
                )
            ]
        )

        handle = client.try_acquire_many(["a", "b"], ttl_ms=1000)

        self.assertIsNone(handle)
        self.assertFalse(client.frames[0]["wait"])

    def test_acquire_many_sends_wait_true_and_returns_grant(self):
        client = StubClient(
            [
                Response.from_dict(
                    {
                        "type": "compositeLock",
                        "uuid": "u",
                        "keys": ["a", "b"],
                        "acquired": True,
                        "lockUuid": "L-1",
                        "fencingTokens": {"a": 1, "b": 2},
                    }
                )
            ]
        )

        handle = client.acquire_many(["a", "b"], ttl_ms=1000)

        self.assertEqual(handle.lock_uuid, "L-1")
        self.assertTrue(client.frames[0]["wait"])


if __name__ == "__main__":
    unittest.main()
