#!/usr/bin/env python3
from pathlib import Path

path = Path('remote/tools/generate-api-docs.mjs')
source = path.read_text()

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
if source.count(lowercase_declaration) != 1:
    raise SystemExit('expected exactly one lowercase OpenAPI method declaration to repair')
source = source.replace(lowercase_declaration, replacement_declaration, 1)

start = source.index('function extractOpenApiRoutes(')
end = source.index('function extractAxumRoutesFromSource(', start)
extractor = source[start:end]
old_guard = 'if (!OPENAPI_METHODS.has(method)) continue;'
new_guard = 'if (!OPENAPI_DOCUMENT_METHODS.has(method)) continue;'
if extractor.count(old_guard) != 1:
    raise SystemExit('expected exactly one OpenAPI document method guard to repair')
extractor = extractor.replace(old_guard, new_guard, 1)
source = source[:start] + extractor + source[end:]

if source.count('const OPENAPI_METHODS = new Set(') != 1:
    raise SystemExit('uppercase runtime OpenAPI method declaration must remain unique')
if source.count('const OPENAPI_DOCUMENT_METHODS = new Set(') != 1:
    raise SystemExit('lowercase document OpenAPI method declaration must be unique')

path.write_text(source)
print('DEN-445 generator identifier collision repaired')
