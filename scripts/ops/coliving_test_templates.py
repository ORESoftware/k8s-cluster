"""Deterministic Python acceptance-test templates."""
from __future__ import annotations
import textwrap
from coliving_repository_specs import RepoSpec

def python_reference_model() -> str:
    return textwrap.dedent(
        """\
        from __future__ import annotations

        from dataclasses import dataclass
        from datetime import datetime
        from typing import Iterable, Mapping

        FORBIDDEN_SOLVER_FIELDS = (
            "name", "email", "phone", "contact", "payment", "stripe", "agreement",
            "legal", "chat", "health", "medical", "allergy", "dietary", "document",
        )

        class ContractViolation(ValueError):
            pass

        def require(condition: bool, message: str) -> None:
            if not condition:
                raise ContractViolation(message)

        def canonical_store(store: str) -> bool:
            return store == "neon_postgres"

        def validate_refund(settled_minor: int, refunded_minor: int) -> None:
            require(isinstance(settled_minor, int) and settled_minor >= 0, "settled amount is invalid")
            require(isinstance(refunded_minor, int) and refunded_minor >= 0, "refund amount is invalid")
            require(refunded_minor <= settled_minor, "refund exceeds settlement")

        def validate_agreement(record: Mapping[str, object]) -> None:
            digest = record.get("documentSha256")
            require(isinstance(digest, str) and len(digest) == 64 and all(c in "0123456789abcdef" for c in digest), "bad document digest")
            require(bool(record.get("serverReceiptId")), "accepted agreement requires server receipt")
            require(bool(record.get("subject")), "accepted agreement requires subject")
            require(bool(record.get("agreementVersion")), "accepted agreement requires version")

        def validate_solver_payload(value: object, path: tuple[str, ...] = ()) -> None:
            if isinstance(value, Mapping):
                for raw_key, child in value.items():
                    key = str(raw_key)
                    lowered = key.lower()
                    require(not any(word in lowered for word in FORBIDDEN_SOLVER_FIELDS), f"protected solver field at {'.'.join(path + (key,))}")
                    validate_solver_payload(child, path + (key,))
            elif isinstance(value, list):
                for index, child in enumerate(value):
                    validate_solver_payload(child, path + (str(index),))

        def validate_reservation(start: str, end: str) -> None:
            left = datetime.fromisoformat(start.replace("Z", "+00:00"))
            right = datetime.fromisoformat(end.replace("Z", "+00:00"))
            require(right > left, "reservation end must be after start")

        def validate_no_overlap(records: Iterable[Mapping[str, object]]) -> None:
            grouped: dict[tuple[object, object, object], list[Mapping[str, object]]] = {}
            for record in records:
                if record.get("status") not in {"requested", "confirmed"}:
                    continue
                key = (record.get("tenantId"), record.get("propertyId"), record.get("resourceId"))
                grouped.setdefault(key, []).append(record)
            for key, group in grouped.items():
                ordered = sorted(group, key=lambda item: str(item["startsAt"]))
                for left, right in zip(ordered, ordered[1:]):
                    require(str(left["endsAt"]) <= str(right["startsAt"]), f"overlap for {key}")

        @dataclass(frozen=True)
        class JobReceipt:
            idempotency_key: str
            attempt: int
            state: str

            def validate(self) -> None:
                require(len(self.idempotency_key) >= 16, "short idempotency key")
                require(self.attempt >= 1, "attempt must be positive")
                require(self.state in {"succeeded", "retryable", "dead_lettered"}, "unknown state")
        """
    )


def python_tests(spec: RepoSpec) -> str:
    lines = [
        "import unittest",
        "",
        "from reference_model import (",
        "    ContractViolation,",
        "    JobReceipt,",
        "    canonical_store,",
        "    validate_agreement,",
        "    validate_no_overlap,",
        "    validate_refund,",
        "    validate_reservation,",
        "    validate_solver_payload,",
        ")",
        "",
        "",
        "class ResidentOperationsTests(unittest.TestCase):",
        "    def test_refund_never_exceeds_settlement(self) -> None:",
        "        validate_refund(85_000, 20_000)",
        "        with self.assertRaises(ContractViolation):",
        "            validate_refund(85_000, 85_001)",
        "",
        "    def test_reservation_interval_is_forward(self) -> None:",
        '        validate_reservation("2026-09-08T17:00:00Z", "2026-09-08T18:00:00Z")',
        "        with self.assertRaises(ContractViolation):",
        '            validate_reservation("2026-09-08T18:00:00Z", "2026-09-08T17:00:00Z")',
    ]
    name = spec.name
    if "payment" in name:
        lines.extend([
            "",
            "    def test_payment_webhook_idempotency_key_is_stable(self) -> None:",
            '        receipt = JobReceipt("stripe:event:evt_0001", 1, "succeeded")',
            "        receipt.validate()",
            "        with self.assertRaises(ContractViolation):",
            '            JobReceipt("short", 1, "succeeded").validate()',
        ])
    if "solver" in name:
        lines.extend([
            "",
            "    def test_solver_payload_is_pseudonymous(self) -> None:",
            '        validate_solver_payload({"candidateIds": ["candidate:1"], "resourceIds": ["room:A"]})',
            "        with self.assertRaises(ContractViolation):",
            '            validate_solver_payload({"residentEmail": "resident@example.test"})',
            "        with self.assertRaises(ContractViolation):",
            '            validate_solver_payload({"constraints": {"dietaryNotes": "private"}})',
        ])
    if "legal" in name:
        lines.extend([
            "",
            "    def test_accepted_agreement_requires_exact_digest_and_server_receipt(self) -> None:",
            "        record = {",
            '            "documentSha256": "a" * 64,',
            '            "serverReceiptId": "receipt:1",',
            '            "subject": "shared-auth|resident:1",',
            '            "agreementVersion": "2026-09-06",',
            "        }",
            "        validate_agreement(record)",
            '        broken = dict(record, documentSha256="bad")',
            "        with self.assertRaises(ContractViolation):",
            "            validate_agreement(broken)",
        ])
    if "sync" in name or "reconciliation" in name:
        lines.extend([
            "",
            "    def test_only_server_postgres_is_canonical(self) -> None:",
            '        self.assertTrue(canonical_store("neon_postgres"))',
            '        for store in ("local_storage", "indexed_db", "supabase"):',
            "            self.assertFalse(canonical_store(store))",
        ])
    if "booking" in name or "platform" in name or "guest" in name or "e2e" in name:
        lines.extend([
            "",
            "    def test_active_reservations_do_not_overlap(self) -> None:",
            '        first = {"tenantId": "t", "propertyId": "p", "resourceId": "kitchen", "status": "confirmed", "startsAt": "2026-09-08T17:00:00Z", "endsAt": "2026-09-08T19:00:00Z"}',
            '        second = dict(first, startsAt="2026-09-08T19:00:00Z", endsAt="2026-09-08T20:00:00Z")',
            "        validate_no_overlap([first, second])",
            '        second = dict(second, startsAt="2026-09-08T18:30:00Z")',
            "        with self.assertRaises(ContractViolation):",
            "            validate_no_overlap([first, second])",
        ])
    lines.extend(["", "", 'if __name__ == "__main__":', "    unittest.main()", ""])
    return "\n".join(lines)

