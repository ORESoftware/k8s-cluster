#!/usr/bin/env python3
from pathlib import Path


def replace_once(source: str, old: str, new: str, *, label: str) -> str:
    if source.count(old) != 1:
        raise SystemExit(f'{label}: expected exactly one match, found {source.count(old)}')
    return source.replace(old, new, 1)


generator_path = Path('remote/tools/generate-api-docs.mjs')
generator = generator_path.read_text()

lowercase_declaration = """const OPENAPI_METHODS = new Set([
  'get',
  'post',
  'put',
  'patch',
  'delete',
  'head',
  'options',
  'trace',
]);"""
replacement_declaration = lowercase_declaration.replace(
    'const OPENAPI_METHODS', 'const OPENAPI_DOCUMENT_METHODS', 1
)
generator = replace_once(
    generator,
    lowercase_declaration,
    replacement_declaration,
    label='lowercase OpenAPI method declaration',
)

start = generator.index('function extractOpenApiRoutes(')
end = generator.index('function extractAxumRoutesFromSource(', start)
extractor = generator[start:end]
extractor = replace_once(
    extractor,
    'if (!OPENAPI_METHODS.has(method)) continue;',
    'if (!OPENAPI_DOCUMENT_METHODS.has(method)) continue;',
    label='OpenAPI document method guard',
)
generator = generator[:start] + extractor + generator[end:]

if generator.count('const OPENAPI_METHODS = new Set(') != 1:
    raise SystemExit('uppercase runtime OpenAPI method declaration must remain unique')
if generator.count('const OPENAPI_DOCUMENT_METHODS = new Set(') != 1:
    raise SystemExit('lowercase document OpenAPI method declaration must be unique')
generator_path.write_text(generator)

main_path = Path('remote/deployments/dd-embeddings-rs/src/main.rs')
main = main_path.read_text()
main = replace_once(
    main,
    'use axum::{middleware, Extension, Json, Router};',
    'use axum::{middleware, Extension, Json};',
    label='unused Axum Router import',
)
main = replace_once(
    main,
    '    use axum::response::IntoResponse;\n',
    '',
    label='unused test IntoResponse import',
)
main_path.write_text(main)

error_path = Path('remote/deployments/dd-embeddings-rs/src/error.rs')
error = error_path.read_text()
error = replace_once(
    error,
    '#[derive(utoipa::IntoResponses)]\npub enum ApiErrorResponses {',
    '#[allow(dead_code)] // Variants are consumed as response metadata by utoipa.\n#[derive(utoipa::IntoResponses)]\npub enum ApiErrorResponses {',
    label='utoipa response metadata dead-code annotation',
)
error_path.write_text(error)

print('DEN-445 generator collision and Rust warning cleanup applied')
