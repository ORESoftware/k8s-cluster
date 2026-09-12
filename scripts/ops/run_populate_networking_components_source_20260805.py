#!/usr/bin/env python3
"""Run the reviewed NCC source materializer with slash-preserving ref encoding."""

from __future__ import annotations

import importlib.util
import sys
import urllib.parse
from pathlib import Path

materializer_path = Path(__file__).with_name("populate_networking_components_source_20260805.py")
spec = importlib.util.spec_from_file_location("ncc_source_materializer_20260805", materializer_path)
if spec is None or spec.loader is None:
    raise RuntimeError(f"unable to load materializer from {materializer_path}")
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

# GitHub refs contain literal slashes (for example, heads/main and
# heads/agent/ncc-source-carrier-20260805). Preserve those separators while
# escaping any other unsafe characters.
module.encoded = lambda value: urllib.parse.quote(value, safe="/")

try:
    raise SystemExit(module.main())
except Exception as error:
    print(f"SOURCE_POPULATION_FAILED {type(error).__name__}: {error}", file=sys.stderr, flush=True)
    raise
