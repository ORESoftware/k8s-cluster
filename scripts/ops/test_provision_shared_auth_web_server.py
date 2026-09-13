#!/usr/bin/env python3
from __future__ import annotations

import io
import json
from pathlib import Path
import stat
import sys
import tarfile
import tempfile
import unittest
import zipfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import provision_shared_auth_web_server as p  # noqa: E402

REQUEST = HERE.parents[1] / "ops/requests/shared-auth-web-server.json"


class RequestTests(unittest.TestCase):
    def test_exact_request_validates(self) -> None:
        request = p.load_request(REQUEST)
        self.assertFalse(request["execute"])

    def test_target_drift_is_rejected(self) -> None:
        data = json.loads(REQUEST.read_text())
        data["target"]["repository"] = "other"
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "request.json"
            path.write_text(json.dumps(data))
            with self.assertRaises(p.ProvisioningError):
                p.load_request(path)

    def test_canary_drift_is_rejected(self) -> None:
        data = json.loads(REQUEST.read_text())
        data["canary"]["commit"] = "0" * 40
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "request.json"
            path.write_text(json.dumps(data))
            with self.assertRaises(p.ProvisioningError):
                p.load_request(path)


class ArchiveTests(unittest.TestCase):
    def test_zip_traversal_is_rejected(self) -> None:
        payload = io.BytesIO()
        with zipfile.ZipFile(payload, "w") as archive:
            archive.writestr("../escape", b"x")
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(p.ProvisioningError):
                p.safe_extract_zip(payload.getvalue(), Path(directory) / "out")

    def test_zip_symlink_is_rejected(self) -> None:
        payload = io.BytesIO()
        info = zipfile.ZipInfo("link")
        info.create_system = 3
        info.external_attr = (stat.S_IFLNK | 0o777) << 16
        with zipfile.ZipFile(payload, "w") as archive:
            archive.writestr(info, b"target")
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(p.ProvisioningError):
                p.safe_extract_zip(payload.getvalue(), Path(directory) / "out")

    def test_tar_traversal_is_rejected_by_candidate_loader(self) -> None:
        manifest = {
            **p.load_request(REQUEST)["candidate"],
            "archive": {"name": "shared-auth-web-server-candidate.tar.gz"},
            "cargo_lock_sha256": "0" * 64,
            "file_count": 1,
            "uncompressed_bytes": 1,
            "files": [
                {
                    "path": "../escape",
                    "sha256": "0" * 64,
                    "bytes": 1,
                    "mode": "0644",
                }
            ],
        }
        tar_bytes = io.BytesIO()
        with tarfile.open(fileobj=tar_bytes, mode="w:gz") as archive:
            info = tarfile.TarInfo("../escape")
            info.size = 1
            archive.addfile(info, io.BytesIO(b"x"))
        raw_tar = tar_bytes.getvalue()
        manifest["archive"].update(
            {"sha256": p.sha256_bytes(raw_tar), "bytes": len(raw_tar)}
        )
        artifact = io.BytesIO()
        with zipfile.ZipFile(artifact, "w") as archive:
            archive.writestr(
                "shared-auth-web-server-candidate.json",
                json.dumps(manifest),
            )
            archive.writestr("shared-auth-web-server-candidate.tar.gz", raw_tar)
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(p.ProvisioningError):
                p.extract_candidate(
                    artifact.getvalue(),
                    Path(directory),
                    p.load_request(REQUEST)["candidate"],
                )


class IdentityTests(unittest.TestCase):
    def test_git_blob_identity(self) -> None:
        self.assertEqual(
            p.git_blob_sha(b"hello\n"),
            "ce013625030ba8dba906f756967f9e9ca394464a",
        )

    def test_redirect_rejects_http(self) -> None:
        handler = p.SafeGithubRedirectHandler()
        request = p.urllib.request.Request(
            "https://api.github.com/source",
            headers={"Authorization": "Bearer secret"},
        )
        with self.assertRaises(p.ProvisioningError):
            handler.redirect_request(
                request,
                None,
                302,
                "Found",
                {},
                "http://example.test/file",
            )


if __name__ == "__main__":
    unittest.main()
