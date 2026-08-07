#!/usr/bin/env python3
from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    target = Path(path)
    text = target.read_text(encoding="utf-8")
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one replacement target, found {count}")
    target.write_text(text.replace(old, new, 1), encoding="utf-8")


replace_once(
    "tools/gitops_composition.py",
    '''    diagnostics.sort(
        key=lambda item: (
''',
    '''    if record_count == 0:
        diagnostics.append(
            Diagnostic(
                "catalog.empty",
                "no catalog records matched the configured catalog glob",
                "catalog/gitops/apps",
            )
        )

    diagnostics.sort(
        key=lambda item: (
''',
)

replace_once(
    "tools/gitops_composition.py",
    '''def main(argv: Sequence[str] | None = None) -> int:
    args = _build_parser().parse_args(argv)
    root = args.root.resolve()
    loaded = load_records(root, args.catalog_glob)

    if args.command == "render":
        print(
            json.dumps(
                {
                    "apiVersion": API_VERSION,
                    "kind": "GitOpsApplicationPreviewList",
                    "items": render_records(loaded),
                },
                indent=2,
                sort_keys=True,
            )
        )
        return 0

    report = validate_records(
        loaded,
        root=root,
        gitmodules=load_gitmodules(root),
        gitlinks=tracked_gitlinks(root),
        strict=not args.no_strict,
    )
    if args.format == "json":
        print(json.dumps(report.to_json(), indent=2, sort_keys=True))
    else:
        print_human(report)
    return 0 if report.valid else 2
''',
    '''def main(argv: Sequence[str] | None = None) -> int:
    args = _build_parser().parse_args(argv)
    root = args.root.resolve()
    loaded = load_records(root, args.catalog_glob)
    report = validate_records(
        loaded,
        root=root,
        gitmodules=load_gitmodules(root),
        gitlinks=tracked_gitlinks(root),
        strict=not getattr(args, "no_strict", False),
    )

    if args.command == "render":
        if not report.valid:
            print_human(report)
            return 2
        print(
            json.dumps(
                {
                    "apiVersion": API_VERSION,
                    "kind": "GitOpsApplicationPreviewList",
                    "items": render_records(loaded),
                },
                indent=2,
                sort_keys=True,
            )
        )
        return 0

    if args.format == "json":
        print(json.dumps(report.to_json(), indent=2, sort_keys=True))
    else:
        print_human(report)
    return 0 if report.valid else 2
''',
)

tests = Path("tools/test_gitops_composition.py")
text = tests.read_text(encoding="utf-8")
if text.count("import copy\nimport json") != 1:
    raise SystemExit("test import target missing or duplicated")
text = text.replace(
    "import copy\nimport json",
    "import contextlib\nimport copy\nimport io\nimport json",
    1,
)
if text.count("    load_gitmodules,\n    load_records,") != 1:
    raise SystemExit("main import target missing or duplicated")
text = text.replace(
    "    load_gitmodules,\n    load_records,",
    "    load_gitmodules,\n    load_records,\n    main,",
    1,
)
marker = "    def test_unknown_fields_fail_in_strict_mode(self):\n"
if text.count(marker) != 1:
    raise SystemExit("test insertion marker missing or duplicated")
additions = '''    def test_empty_catalog_fails_closed(self):
        report = validate_records(
            [],
            root=self.root,
            gitmodules={},
            gitlinks={},
        )
        rules = {item.rule_id for item in report.diagnostics}
        self.assertIn("catalog.empty", rules)
        self.assertFalse(report.valid)

    def test_render_command_rejects_invalid_catalog(self):
        broken = record()
        broken["spec"]["source"]["targetRevision"] = "a" * 40
        self.write_record(broken)
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            status = main(["render", "--root", str(self.root)])
        self.assertEqual(2, status)
        self.assertIn("source.pin-drift", output.getvalue())
        self.assertNotIn("GitOpsApplicationPreviewList", output.getvalue())

'''
tests.write_text(text.replace(marker, additions + marker, 1), encoding="utf-8")
