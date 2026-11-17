from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).resolve().parents[1] / "scripts/ci/verify_ghcr_manifest_digests.py"
SPEC = importlib.util.spec_from_file_location("ghcr_preflight", MODULE_PATH)
assert SPEC and SPEC.loader
module = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = module
SPEC.loader.exec_module(module)

ImageReference = module.ImageReference
Response = module.Response
VerificationError = module.VerificationError


class FakeTransport:
    def __init__(self, responses):
        self.responses = list(responses)
        self.requests = []

    def request(self, **kwargs):
        self.requests.append(kwargs)
        if not self.responses:
            raise AssertionError("unexpected request")
        return self.responses.pop(0)


def token_response(token="A" * 32):
    return Response(
        status=200,
        headers={"content-type": "application/json"},
        body=json.dumps({"token": token}).encode(),
    )


class GhcrManifestPreflightTests(unittest.TestCase):
    def reference(self):
        return ImageReference(
            full="ghcr.io/oresoftware/example@sha256:" + "a" * 64,
            repository="oresoftware/example",
            digest="sha256:" + "a" * 64,
        )

    def test_manifest_parser_deduplicates_exact_refs(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "deployment.yaml"
            image = self.reference().full
            path.write_text(f"image: {image}\n  image: {image}\n", encoding="utf-8")
            self.assertEqual(module.collect_references([path]), [self.reference()])

    def test_mutable_ghcr_tag_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "deployment.yaml"
            path.write_text("image: ghcr.io/oresoftware/example:latest\n", encoding="utf-8")
            with self.assertRaisesRegex(VerificationError, "not an exact"):
                module.collect_references([path])

    def test_exact_manifest_succeeds_without_serializing_bearer(self):
        reference = self.reference()
        secret = "S" * 40
        transport = FakeTransport(
            [
                token_response(secret),
                Response(200, {"Docker-Content-Digest": reference.digest}),
            ]
        )
        result = module.verify_reference(reference, transport)
        self.assertTrue(result["digest_header_match"])
        self.assertNotIn(secret, json.dumps(result))
        self.assertEqual(transport.requests[1]["method"], "HEAD")
        self.assertEqual(transport.requests[1]["max_body_bytes"], 0)
        self.assertEqual(transport.requests[1]["headers"]["Authorization"], f"Bearer {secret}")

    def test_missing_manifest_is_rejected(self):
        reference = self.reference()
        transport = FakeTransport([token_response(), Response(404, {})])
        with self.assertRaisesRegex(VerificationError, "HTTP 404"):
            module.verify_reference(reference, transport)

    def test_redirect_is_rejected(self):
        reference = self.reference()
        transport = FakeTransport(
            [token_response(), Response(302, {"Location": "https://example.invalid"})]
        )
        with self.assertRaisesRegex(VerificationError, "HTTP 302"):
            module.verify_reference(reference, transport)

    def test_digest_mismatch_is_rejected_without_leaking_observed_value(self):
        reference = self.reference()
        other = "sha256:" + "b" * 64
        transport = FakeTransport(
            [token_response(), Response(200, {"Docker-Content-Digest": other})]
        )
        with self.assertRaisesRegex(VerificationError, "mismatched") as raised:
            module.verify_reference(reference, transport)
        self.assertNotIn(other, str(raised.exception))

    def test_missing_digest_header_is_rejected(self):
        reference = self.reference()
        transport = FakeTransport([token_response(), Response(200, {})])
        with self.assertRaisesRegex(VerificationError, "missing"):
            module.verify_reference(reference, transport)

    def test_malformed_or_unbounded_token_is_rejected(self):
        reference = self.reference()
        for body in (b"not-json", json.dumps({"token": "tiny"}).encode()):
            with self.subTest(body=body):
                transport = FakeTransport([Response(200, {}, body)])
                with self.assertRaises(VerificationError):
                    module.verify_reference(reference, transport)

    def test_audit_is_mode_0600_and_content_free(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "audit.json"
            module.write_audit(path, {"passed": False, "error": "HTTP 404"})
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            self.assertEqual(
                json.loads(path.read_text()),
                {"passed": False, "error": "HTTP 404"},
            )


if __name__ == "__main__":
    unittest.main()
