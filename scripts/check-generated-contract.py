#!/usr/bin/env python3
"""Runtime contract checker for `generated/` trees.

Compile-time types (Dart/TS/Rust/…) are not enough. This checker proves the
contract still holds at runtime:

1. Policy README is present (`<!-- generated-policy: frozen|writable -->`).
2. Frozen generated files can be made read-only (`chmod a-w`); git does not
   persist the write bit, so `--freeze` reapplies it after checkout.
3. JSON Schema (Draft 2020-12 when `jsonschema` is installed) validates
   fixtures: `valid` instances must pass, `invalid` instances must fail.
4. JSON Schema is a **cross-check**, not necessarily the primary codegen
   input. Primary sources remain `.cli-flags.toml`, route-map JSON, OpenAPI,
   etc. Schema properties are compared to those sources when both exist.

Usage:
    python3 scripts/check-generated-contract.py
    python3 scripts/check-generated-contract.py --freeze --require-readonly
    python3 scripts/check-generated-contract.py --self-test
"""

from __future__ import annotations

import argparse
import json
import os
import re
import stat
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Any, Iterable

POLICY_FROZEN = "frozen"
POLICY_WRITABLE = "writable"
POLICY_RE = re.compile(
    r"<!--\s*generated-policy:\s*(frozen|writable)\s*-->", re.IGNORECASE
)
README_NAMES = ("README.md", "readme.md")
SKIP_DIR_NAMES = {
    ".git",
    "node_modules",
    "target",
    "vendor",
    "build",
    "dist",
    ".dart_tool",
    "__pycache__",
    "tmp",
    "_to_delete",
    ".staging",
}
SCHEMA_DIR_NAMES = ("schema", "schemas", "json-schema")
ENV_KEY_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")
TS_FIELD_RE = re.compile(
    r"^\s*(?:readonly\s+)?([A-Za-z_][A-Za-z0-9_]*)\??\s*:", re.MULTILINE
)
DART_FIELD_RE = re.compile(
    r"^\s*final\s+[^=;]+?\s+([A-Za-z_][A-Za-z0-9_]*)\s*;", re.MULTILINE
)


def repo_root_from_script() -> Path:
    here = Path(__file__).resolve()
    if here.parent.name == "scripts":
        return here.parent.parent
    return Path.cwd()


def is_readme(path: Path) -> bool:
    return path.name in README_NAMES


def walk_dirs(root: Path) -> Iterable[Path]:
    for dirpath, dirnames, _filenames in os.walk(root):
        dirnames[:] = [
            name
            for name in dirnames
            if name not in SKIP_DIR_NAMES and not name.startswith(".")
        ]
        yield Path(dirpath)


def find_generated_dirs(root: Path) -> list[Path]:
    found: list[Path] = []
    for directory in walk_dirs(root):
        if directory.name == "generated":
            found.append(directory)
    return found


def read_policy(generated_dir: Path) -> str | None:
    for name in README_NAMES:
        readme = generated_dir / name
        if not readme.is_file():
            continue
        text = readme.read_text(encoding="utf-8")
        match = POLICY_RE.search(text)
        if match:
            return match.group(1).lower()
    return None


def generated_files(generated_dir: Path) -> list[Path]:
    files: list[Path] = []
    for dirpath, dirnames, filenames in os.walk(generated_dir):
        dirnames[:] = [
            name
            for name in dirnames
            if name not in SKIP_DIR_NAMES and not name.startswith(".")
        ]
        for name in filenames:
            path = Path(dirpath) / name
            if path.is_file():
                files.append(path)
    return files


def is_writable(path: Path) -> bool:
    mode = path.stat().st_mode
    return bool(mode & (stat.S_IWUSR | stat.S_IWGRP | stat.S_IWOTH))


def freeze_file(path: Path) -> None:
    if is_readme(path):
        return
    mode = path.stat().st_mode
    path.chmod(mode & ~0o222)


def thaw_file(path: Path) -> None:
    mode = path.stat().st_mode
    path.chmod(mode | stat.S_IWUSR)


def freeze_tree(generated_dir: Path) -> int:
    count = 0
    for path in generated_files(generated_dir):
        if is_readme(path) or path.is_symlink():
            continue
        freeze_file(path)
        count += 1
    return count


def discover_schemas(root: Path) -> list[Path]:
    schemas: list[Path] = []
    for directory in walk_dirs(root):
        if directory.name in SCHEMA_DIR_NAMES or directory.name == "generated":
            for path in directory.rglob("*.json"):
                if not path.is_file():
                    continue
                if any(part in SKIP_DIR_NAMES for part in path.parts):
                    continue
                name = path.name.lower()
                if name.endswith(".schema.json") or "schema" in name:
                    schemas.append(path)
                elif directory.name in SCHEMA_DIR_NAMES and name.endswith(".json"):
                    schemas.append(path)
    unique: list[Path] = []
    seen: set[Path] = set()
    for path in schemas:
        resolved = path.resolve()
        if resolved in seen:
            continue
        seen.add(resolved)
        unique.append(path)
    return unique


def load_json(path: Path) -> Any | None:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None


def looks_like_schema(doc: Any) -> bool:
    if not isinstance(doc, dict):
        return False
    if "$schema" in doc or "$id" in doc or "properties" in doc or "$defs" in doc:
        return True
    return doc.get("type") in {"object", "array", "string", "number", "integer", "boolean"}


def jsonschema_validate(instance: Any, schema: dict[str, Any]) -> list[str]:
    try:
        import jsonschema
    except ImportError:
        return structural_validate(instance, schema)
    validator_cls = getattr(jsonschema, "Draft202012Validator", jsonschema.Draft7Validator)
    try:
        validator = validator_cls(schema)
        return [
            f"{list(error.absolute_path)}: {error.message}"
            for error in validator.iter_errors(instance)
        ]
    except Exception as exc:  # noqa: BLE001 — surface validator construction failures
        return [f"schema rejected by validator: {exc}"]


def structural_validate(instance: Any, schema: dict[str, Any]) -> list[str]:
    """Subset used when the `jsonschema` package is not installed."""
    errors: list[str] = []
    if schema.get("type") == "object" and not isinstance(instance, dict):
        return ["instance is not an object"]
    if not isinstance(instance, dict) or not isinstance(schema.get("properties"), dict):
        return errors
    required = schema.get("required") or []
    if isinstance(required, list):
        for key in required:
            if key not in instance:
                errors.append(f"missing required property {key!r}")
    additional = schema.get("additionalProperties", True)
    if additional is False:
        allowed = set(schema["properties"])
        for key in instance:
            if key not in allowed:
                errors.append(f"undeclared property {key!r}")
    return errors


def fixture_dirs(root: Path, generated_dir: Path) -> tuple[list[Path], list[Path]]:
    candidates = [
        generated_dir / "fixtures" / "valid",
        generated_dir / "fixtures" / "invalid",
        root / "tests" / "generated-contract" / "valid",
        root / "tests" / "generated-contract" / "invalid",
        root / "schema" / "examples",
        root / "schemas" / "examples",
    ]
    valid: list[Path] = []
    invalid: list[Path] = []
    for path in candidates:
        if not path.is_dir():
            continue
        if path.name == "invalid" or path.parent.name == "invalid":
            invalid.append(path)
        else:
            valid.append(path)
    return valid, invalid


def json_files(directory: Path) -> list[Path]:
    return sorted(path for path in directory.glob("*.json") if path.is_file())


def schema_examples(schema: dict[str, Any]) -> list[Any]:
    examples = schema.get("examples")
    if isinstance(examples, list):
        return examples
    return []


def load_toml(path: Path) -> dict[str, Any] | None:
    try:
        import tomllib
    except ImportError:
        try:
            import tomli as tomllib  # type: ignore
        except ImportError:
            return None
    try:
        with path.open("rb") as handle:
            data = tomllib.load(handle)
    except (OSError, ValueError):
        return None
    return data if isinstance(data, dict) else None


def walk_cli_flags(data: dict[str, Any]) -> dict[str, dict[str, Any]]:
    envs: dict[str, dict[str, Any]] = {}

    def walk_flags(flags: Any) -> None:
        if not isinstance(flags, dict):
            return
        for spec in flags.values():
            if isinstance(spec, dict) and isinstance(spec.get("env"), str):
                envs[spec["env"]] = spec

    def walk_commands(commands: Any) -> None:
        if not isinstance(commands, dict):
            return
        for spec in commands.values():
            if not isinstance(spec, dict):
                continue
            walk_flags(spec.get("flags"))
            walk_commands(spec.get("commands"))

    walk_flags(data.get("flags"))
    walk_commands(data.get("commands"))
    return envs


def coerce_default(spec: dict[str, Any]) -> Any:
    if "default" not in spec:
        return None
    value = spec["default"]
    declared = str(spec.get("type") or "string").lower()
    if declared in {"bool", "boolean"}:
        if isinstance(value, bool):
            return value
        return str(value).lower() in {"1", "true", "yes", "on"}
    if declared in {"integer", "int"}:
        try:
            return int(value)
        except (TypeError, ValueError):
            return value
    if declared in {"double", "float", "number"}:
        try:
            return float(value)
        except (TypeError, ValueError):
            return value
    if declared in {"json", "array", "map", "object"} and isinstance(value, str):
        try:
            return json.loads(value)
        except json.JSONDecodeError:
            return value
    return value


def instance_from_cli_flags(envs: dict[str, dict[str, Any]]) -> dict[str, Any]:
    instance: dict[str, Any] = {}
    for env_name, spec in envs.items():
        if "default" in spec:
            instance[env_name] = coerce_default(spec)
    return instance


def keys_from_generated_types(generated_dir: Path) -> set[str]:
    keys: set[str] = set()
    for path in generated_files(generated_dir):
        if path.suffix not in {".ts", ".dart", ".rs", ".go", ".py"}:
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        if path.suffix == ".ts":
            keys.update(TS_FIELD_RE.findall(text))
        elif path.suffix == ".dart":
            keys.update(DART_FIELD_RE.findall(text))
        else:
            keys.update(
                name for name in ENV_KEY_RE.findall(text) if name.isupper() or "_" in name
            )
    return {key for key in keys if ENV_KEY_RE.match(key)}


def schema_property_names(schema: dict[str, Any]) -> set[str]:
    properties = schema.get("properties")
    if isinstance(properties, dict):
        return {str(key) for key in properties}
    return set()


class Findings:
    def __init__(self) -> None:
        self.errors: list[str] = []
        self.warnings: list[str] = []
        self.notes: list[str] = []

    def error(self, message: str) -> None:
        self.errors.append(message)

    def warn(self, message: str) -> None:
        self.warnings.append(message)

    def note(self, message: str) -> None:
        self.notes.append(message)


def check_generated_dir(
    root: Path,
    generated_dir: Path,
    findings: Findings,
    *,
    freeze: bool,
    require_readonly: bool,
) -> str | None:
    policy = read_policy(generated_dir)
    rel = generated_dir.relative_to(root)
    if policy is None:
        findings.error(
            f"{rel}: missing policy README with "
            "`<!-- generated-policy: frozen -->` or "
            "`<!-- generated-policy: writable -->`"
        )
        return None
    findings.note(f"{rel}: policy={policy}")
    if policy == POLICY_WRITABLE:
        if freeze:
            findings.note(f"{rel}: writable tree left unchanged")
        return policy
    if freeze:
        freeze_tree(generated_dir)
        findings.note(f"{rel}: froze generated files (chmod a-w, README kept writable)")
    if require_readonly:
        for path in generated_files(generated_dir):
            if is_readme(path) or path.is_symlink():
                continue
            if is_writable(path):
                findings.error(
                    f"{path.relative_to(root)}: frozen generated file is writable; "
                    "re-run with --freeze or chmod a-w after regenerate"
                )
    return policy


def check_schema_runtime(root: Path, findings: Findings) -> None:
    schemas = [path for path in discover_schemas(root) if looks_like_schema(load_json(path))]
    if not schemas:
        findings.note("no JSON Schema documents found to runtime-check")
        return
    validated = 0
    for schema_path in schemas:
        doc = load_json(schema_path)
        if not isinstance(doc, dict):
            continue
        rel = schema_path.relative_to(root)
        for example in schema_examples(doc):
            errors = jsonschema_validate(example, doc)
            validated += 1
            if errors:
                findings.error(f"{rel}: embedded example failed runtime schema check: {errors[0]}")
        sibling_examples = schema_path.parent / "examples"
        if sibling_examples.is_dir():
            for fixture in json_files(sibling_examples):
                instance = load_json(fixture)
                if instance is None:
                    findings.error(f"{fixture.relative_to(root)}: invalid JSON fixture")
                    continue
                errors = jsonschema_validate(instance, doc)
                validated += 1
                if errors:
                    findings.error(
                        f"{fixture.relative_to(root)}: failed {rel}: {errors[0]}"
                    )
    contract_root = root / "tests" / "generated-contract"
    schema_for_fixtures = None
    for candidate in (
        root / "generated" / "json-schema",
        root / "json-schema",
        root / "schema",
        root / "schemas",
    ):
        if candidate.is_dir():
            for path in sorted(candidate.rglob("*.json")):
                doc = load_json(path)
                if looks_like_schema(doc):
                    schema_for_fixtures = (path, doc)
                    break
        if schema_for_fixtures:
            break
    if schema_for_fixtures and contract_root.is_dir():
        schema_path, schema_doc = schema_for_fixtures
        for fixture in json_files(contract_root / "valid"):
            instance = load_json(fixture)
            validated += 1
            if instance is None:
                findings.error(f"{fixture.relative_to(root)}: invalid JSON")
                continue
            errors = jsonschema_validate(instance, schema_doc)
            if errors:
                findings.error(
                    f"{fixture.relative_to(root)} must satisfy {schema_path.relative_to(root)}: {errors[0]}"
                )
        for fixture in json_files(contract_root / "invalid"):
            instance = load_json(fixture)
            validated += 1
            if instance is None:
                continue
            errors = jsonschema_validate(instance, schema_doc)
            if not errors:
                findings.error(
                    f"{fixture.relative_to(root)} must be rejected by {schema_path.relative_to(root)}"
                )
    findings.note(f"runtime JSON Schema checks exercised {validated} instance(s)")


def check_cli_flags_cross(root: Path, findings: Findings) -> None:
    toml_path = root / ".cli-flags.toml"
    if not toml_path.is_file():
        return
    data = load_toml(toml_path)
    if data is None:
        findings.warn(".cli-flags.toml present but tomllib/tomli unavailable or invalid")
        return
    envs = walk_cli_flags(data)
    if not envs:
        findings.note(".cli-flags.toml has no flag env destinations")
        return
    instance = instance_from_cli_flags(envs)
    schema_docs: list[tuple[Path, dict[str, Any]]] = []
    for path in discover_schemas(root):
        doc = load_json(path)
        if isinstance(doc, dict) and isinstance(doc.get("properties"), dict):
            props = set(doc["properties"])
            if props & set(envs):
                schema_docs.append((path, doc))
    for path, doc in schema_docs:
        schema_keys = schema_property_names(doc)
        missing = set(envs) - schema_keys
        extra = schema_keys - set(envs)
        if missing:
            findings.error(
                f"{path.relative_to(root)}: JSON Schema missing env keys from .cli-flags.toml: "
                + ", ".join(sorted(missing)[:12])
            )
        if extra:
            findings.warn(
                f"{path.relative_to(root)}: schema has keys not in .cli-flags.toml: "
                + ", ".join(sorted(extra)[:12])
            )
        if instance:
            errors = jsonschema_validate(instance, doc)
            if errors:
                findings.error(
                    f"{path.relative_to(root)}: defaults from .cli-flags.toml failed runtime schema: {errors[0]}"
                )
            else:
                findings.note(
                    f"{path.relative_to(root)}: .cli-flags.toml defaults satisfy generated/declared schema"
                )
            invalid = dict(instance)
            invalid["UNDECLARED_F2E_CONTRACT_PROBE"] = "no"
            if doc.get("additionalProperties") is False:
                bad = jsonschema_validate(invalid, doc)
                if not bad:
                    findings.error(
                        f"{path.relative_to(root)}: additionalProperties=false did not reject undeclared key"
                    )
    generated_dirs = find_generated_dirs(root)
    for generated_dir in generated_dirs:
        if read_policy(generated_dir) == POLICY_WRITABLE:
            continue
        type_keys = keys_from_generated_types(generated_dir)
        if not type_keys:
            continue
        overlap = type_keys & set(envs)
        if overlap or (type_keys and any(key in text for key in list(envs)[:1] for text in [""])):
            missing = set(envs) - type_keys
            # Only error when the generated file clearly looks like flags-2-env output.
            marker_files = [
                path
                for path in generated_files(generated_dir)
                if "flags2env" in path.read_text(encoding="utf-8", errors="replace").lower()
                or path.name in {"env.dart", "cli-interfaces.ts", "cli_config.rs"}
            ]
            if marker_files and missing:
                findings.error(
                    f"{generated_dir.relative_to(root)}: generated types missing env keys: "
                    + ", ".join(sorted(missing)[:12])
                )
            elif overlap:
                findings.note(
                    f"{generated_dir.relative_to(root)}: generated types overlap .cli-flags.toml on "
                    f"{len(overlap)} env key(s)"
                )


def check_route_map_cross(root: Path, findings: Findings) -> None:
    maps = list(root.glob("*.route-map.json")) + list((root / "examples").glob("*.route-map.json"))
    if not maps:
        return
    schema_path = root / "json-schema" / "route-map.schema.json"
    schema_doc = load_json(schema_path) if schema_path.is_file() else None
    for map_path in maps:
        instance = load_json(map_path)
        if not isinstance(instance, dict):
            findings.error(f"{map_path.relative_to(root)}: route map is not a JSON object")
            continue
        if isinstance(schema_doc, dict):
            errors = jsonschema_validate(instance, schema_doc)
            if errors:
                findings.error(
                    f"{map_path.relative_to(root)} failed route-map schema at runtime: {errors[0]}"
                )
            else:
                findings.note(f"{map_path.relative_to(root)} satisfies json-schema/route-map.schema.json")


def run_checks(root: Path, *, freeze: bool, require_readonly: bool) -> Findings:
    findings = Findings()
    generated_dirs = find_generated_dirs(root)
    if not generated_dirs:
        findings.warn("no generated/ directory found")
    for generated_dir in generated_dirs:
        check_generated_dir(
            root,
            generated_dir,
            findings,
            freeze=freeze,
            require_readonly=require_readonly,
        )
    check_schema_runtime(root, findings)
    check_cli_flags_cross(root, findings)
    check_route_map_cross(root, findings)
    return findings


class SelfTests(unittest.TestCase):
    def test_frozen_tree_and_schema_runtime(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            generated = root / "generated" / "dart"
            generated.mkdir(parents=True)
            (root / "generated" / "README.md").write_text(
                "<!-- generated-policy: frozen -->\n# frozen\n",
                encoding="utf-8",
            )
            env_path = generated / "env.dart"
            env_path.write_text(
                "// Generated by flags2env from .cli-flags.toml. Do not edit.\n"
                "final class Env { final int PORT; Env({required this.PORT}); }\n",
                encoding="utf-8",
            )
            schema = {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "additionalProperties": False,
                "required": ["PORT"],
                "properties": {"PORT": {"type": "integer"}},
                "examples": [{"PORT": 7}],
            }
            schema_dir = root / "generated" / "json-schema"
            schema_dir.mkdir(parents=True)
            (schema_dir / "env.schema.json").write_text(json.dumps(schema), encoding="utf-8")
            valid_dir = root / "tests" / "generated-contract" / "valid"
            invalid_dir = root / "tests" / "generated-contract" / "invalid"
            valid_dir.mkdir(parents=True)
            invalid_dir.mkdir(parents=True)
            (valid_dir / "ok.json").write_text(json.dumps({"PORT": 9}), encoding="utf-8")
            (invalid_dir / "bad.json").write_text(json.dumps({"PORT": "no"}), encoding="utf-8")
            (root / ".cli-flags.toml").write_text(
                '[flags.port]\nenv = "PORT"\ntype = "integer"\ndefault = 9\n',
                encoding="utf-8",
            )
            findings = run_checks(root, freeze=True, require_readonly=True)
            self.assertEqual(findings.errors, [], findings.errors)
            self.assertFalse(is_writable(env_path))
            self.assertTrue(is_writable(root / "generated" / "README.md"))

    def test_writable_policy_skips_chmod(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            generated = root / "generated"
            generated.mkdir()
            (generated / "README.md").write_text(
                "<!-- generated-policy: writable -->\nscratch\n",
                encoding="utf-8",
            )
            blob = generated / "out.wav"
            blob.write_text("x", encoding="utf-8")
            findings = run_checks(root, freeze=True, require_readonly=True)
            self.assertEqual(findings.errors, [], findings.errors)
            self.assertTrue(is_writable(blob))

    def test_invalid_fixture_must_fail_schema(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            generated = root / "generated"
            generated.mkdir()
            (generated / "README.md").write_text(
                "<!-- generated-policy: frozen -->\n",
                encoding="utf-8",
            )
            schema = {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "additionalProperties": False,
                "properties": {"PORT": {"type": "integer"}},
                "required": ["PORT"],
            }
            (root / "json-schema").mkdir()
            (root / "json-schema" / "env.schema.json").write_text(
                json.dumps(schema), encoding="utf-8"
            )
            invalid_dir = root / "tests" / "generated-contract" / "invalid"
            invalid_dir.mkdir(parents=True)
            (invalid_dir / "extra.json").write_text(
                json.dumps({"PORT": 1, "NOPE": True}), encoding="utf-8"
            )
            # This extra key should be rejected; if schema is too loose the checker errors.
            findings = run_checks(root, freeze=False, require_readonly=False)
            # additionalProperties false + extra key => invalid fixture correctly rejected (no error)
            self.assertEqual(findings.errors, [], findings.errors)


def print_findings(findings: Findings) -> int:
    for line in findings.notes:
        print(f"note: {line}")
    for line in findings.warnings:
        print(f"warning: {line}", file=sys.stderr)
    for line in findings.errors:
        print(f"error: {line}", file=sys.stderr)
    if findings.errors:
        return 1
    print("generated-contract: ok")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=None)
    parser.add_argument(
        "--freeze",
        action="store_true",
        help="chmod a-w frozen generated files (README stays writable)",
    )
    parser.add_argument(
        "--require-readonly",
        action="store_true",
        help="fail if a frozen generated file is still writable",
    )
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)
    if args.self_test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(SelfTests)
        result = unittest.TextTestRunner(verbosity=2).run(suite)
        return 0 if result.wasSuccessful() else 1
    root = (args.root or repo_root_from_script()).resolve()
    findings = run_checks(root, freeze=args.freeze, require_readonly=args.require_readonly)
    return print_findings(findings)


if __name__ == "__main__":
    raise SystemExit(main())
