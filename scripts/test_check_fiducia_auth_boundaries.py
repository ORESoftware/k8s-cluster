from __future__ import annotations

import contextlib
import importlib.util
import io
import tempfile
import unittest
from pathlib import Path
from unittest import mock

SCRIPT = Path(__file__).with_name("check-fiducia-auth-boundaries.py")
SPEC = importlib.util.spec_from_file_location("fiducia_auth_boundaries", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
BOUNDARIES = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BOUNDARIES)


def external_secret(
    name: str,
    plane: str,
    remote_object: str,
    url_property: str,
    key_property: str,
) -> str:
    return f"""\
apiVersion: external-secrets.io/v1
kind: ExternalSecret
metadata:
  name: {name}
spec:
  target:
    name: {name}
    template:
      metadata:
        labels:
          dd.dev/supabase-plane: {plane}
  data:
    - secretKey: supabase-url
      remoteRef:
        key: {remote_object}
        property: {url_property}
    - secretKey: supabase-publishable-key
      remoteRef:
        key: {remote_object}
        property: {key_property}
"""


def valid_secrets() -> str:
    return "---\n".join(
        [
            external_secret(
                "fiducia-backend-secrets",
                "customer",
                "dd/remote-dev/fiducia-backend-secrets",
                "FIDUCIA_CUSTOMER_SUPABASE_URL",
                "FIDUCIA_CUSTOMER_SUPABASE_PUBLISHABLE_KEY",
            ),
            external_secret(
                "fiducia-admin-secrets",
                "admin",
                "dd/remote-dev/fiducia-admin-secrets",
                "FIDUCIA_ADMIN_SUPABASE_URL",
                "FIDUCIA_ADMIN_SUPABASE_PUBLISHABLE_KEY",
            ),
        ]
    )


def deployment(secret_name: str, *, inline: bool = False) -> str:
    if inline:
        url_ref = (
            f"secretKeyRef: {{ name: {secret_name}, key: supabase-url }}"
        )
        key_ref = (
            "secretKeyRef: { name: "
            f"{secret_name}, key: supabase-publishable-key }}"
        )
    else:
        url_ref = f"""secretKeyRef:
              name: {secret_name}
              key: supabase-url"""
        key_ref = f"""secretKeyRef:
              name: {secret_name}
              key: supabase-publishable-key"""
    return f"""\
apiVersion: apps/v1
kind: Deployment
spec:
  template:
    spec:
      containers:
        - name: app
          env:
            - name: SUPABASE_URL
              valueFrom:
                {url_ref}
            - name: SUPABASE_PUBLISHABLE_KEY
              valueFrom:
                {key_ref}
"""


class ParserTests(unittest.TestCase):
    def test_external_secret_selection_uses_top_level_metadata_name(self) -> None:
        decoy = """\
apiVersion: external-secrets.io/v1
kind: ExternalSecret
metadata:
  name: unrelated
spec:
  target:
    name: fiducia-backend-secrets
"""
        genuine = external_secret(
            "fiducia-backend-secrets",
            "customer",
            "dd/remote-dev/fiducia-backend-secrets",
            "FIDUCIA_CUSTOMER_SUPABASE_URL",
            "FIDUCIA_CUSTOMER_SUPABASE_PUBLISHABLE_KEY",
        )
        selected = BOUNDARIES.external_secret_document(
            f"{decoy}---\n{genuine}", "fiducia-backend-secrets"
        )
        self.assertIn("metadata:\n  name: fiducia-backend-secrets", selected)
        self.assertNotIn("metadata:\n  name: unrelated", selected)

    def test_deployment_secret_ref_accepts_flow_and_block_yaml(self) -> None:
        expected = ("fiducia-admin-secrets", "supabase-url")
        self.assertEqual(
            BOUNDARIES.deployment_secret_ref(
                deployment("fiducia-admin-secrets", inline=True), "SUPABASE_URL"
            ),
            expected,
        )
        self.assertEqual(
            BOUNDARIES.deployment_secret_ref(
                deployment("fiducia-admin-secrets", inline=False), "SUPABASE_URL"
            ),
            expected,
        )

    def test_duplicate_environment_entries_are_rejected(self) -> None:
        duplicate = deployment("fiducia-admin-secrets") + """\
            - name: SUPABASE_URL
              valueFrom:
                secretKeyRef:
                  name: fiducia-backend-secrets
                  key: supabase-url
"""
        with self.assertRaisesRegex(SystemExit, "exactly one SUPABASE_URL"):
            BOUNDARIES.deployment_secret_ref(duplicate, "SUPABASE_URL")

    def test_duplicate_external_secret_data_entries_are_rejected(self) -> None:
        document = external_secret(
            "fiducia-admin-secrets",
            "admin",
            "dd/remote-dev/fiducia-admin-secrets",
            "FIDUCIA_ADMIN_SUPABASE_URL",
            "FIDUCIA_ADMIN_SUPABASE_PUBLISHABLE_KEY",
        ) + """\
    - secretKey: supabase-url
      remoteRef:
        key: dd/remote-dev/fiducia-backend-secrets
        property: FIDUCIA_CUSTOMER_SUPABASE_URL
"""
        with self.assertRaisesRegex(SystemExit, "exactly one data entry"):
            BOUNDARIES.external_secret_ref(document, "supabase-url")

    def test_incomplete_secret_key_reference_is_rejected(self) -> None:
        incomplete = """\
            - name: SUPABASE_URL
              valueFrom:
                secretKeyRef:
                  name: fiducia-admin-secrets
"""
        with self.assertRaisesRegex(SystemExit, "missing key"):
            BOUNDARIES.deployment_secret_ref(incomplete, "SUPABASE_URL")


class EndToEndTests(unittest.TestCase):
    def run_main(
        self,
        secrets: str,
        customer_deployment: str,
        admin_deployment: str,
    ) -> str:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            secrets_path = root / "secrets.yaml"
            customer_path = root / "customer.yaml"
            admin_path = root / "admin.yaml"
            secrets_path.write_text(secrets, encoding="utf-8")
            customer_path.write_text(customer_deployment, encoding="utf-8")
            admin_path.write_text(admin_deployment, encoding="utf-8")
            output = io.StringIO()
            with (
                mock.patch.object(BOUNDARIES, "SECRETS", secrets_path),
                mock.patch.object(BOUNDARIES, "CUSTOMER_DEPLOYMENT", customer_path),
                mock.patch.object(BOUNDARIES, "ADMIN_DEPLOYMENT", admin_path),
                contextlib.redirect_stdout(output),
            ):
                BOUNDARIES.main()
            return output.getvalue()

    def test_main_accepts_distinct_remote_and_kubernetes_planes(self) -> None:
        output = self.run_main(
            valid_secrets(),
            deployment("fiducia-backend-secrets"),
            deployment("fiducia-admin-secrets", inline=True),
        )
        self.assertIn("fiducia auth boundary check passed", output)

    def test_main_rejects_admin_consuming_customer_kubernetes_secret(self) -> None:
        with self.assertRaisesRegex(
            SystemExit, "admin SUPABASE_URL has unexpected source"
        ):
            self.run_main(
                valid_secrets(),
                deployment("fiducia-backend-secrets"),
                deployment("fiducia-backend-secrets"),
            )

    def test_main_rejects_cross_plane_remote_property(self) -> None:
        crossed = "---\n".join(
            [
                external_secret(
                    "fiducia-backend-secrets",
                    "customer",
                    "dd/remote-dev/fiducia-backend-secrets",
                    "FIDUCIA_CUSTOMER_SUPABASE_URL",
                    "FIDUCIA_CUSTOMER_SUPABASE_PUBLISHABLE_KEY",
                ),
                external_secret(
                    "fiducia-admin-secrets",
                    "admin",
                    "dd/remote-dev/fiducia-admin-secrets",
                    "FIDUCIA_CUSTOMER_SUPABASE_URL",
                    "FIDUCIA_ADMIN_SUPABASE_PUBLISHABLE_KEY",
                ),
            ]
        )
        with self.assertRaisesRegex(
            SystemExit, "admin Supabase URL has unexpected remote source"
        ):
            self.run_main(
                crossed,
                deployment("fiducia-backend-secrets"),
                deployment("fiducia-admin-secrets"),
            )


if __name__ == "__main__":
    unittest.main()
