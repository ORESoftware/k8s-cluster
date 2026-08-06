#!/usr/bin/env python3
from __future__ import annotations

import argparse
from pathlib import Path

from dd_ai_ml_pipeline_api import create_contract_app, export_openapi


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Export deterministic ai-ml-pipeline OpenAPI contracts."
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=Path(__file__).resolve().parents[1] / "generated",
    )
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()

    app = create_contract_app()
    public_path, internal_path = export_openapi(app, args.output_dir)
    if args.check:
        first_public = public_path.read_bytes()
        first_internal = internal_path.read_bytes()
        export_openapi(app, args.output_dir)
        if first_public != public_path.read_bytes() or first_internal != internal_path.read_bytes():
            raise SystemExit("OpenAPI export is not deterministic")


if __name__ == "__main__":
    main()
