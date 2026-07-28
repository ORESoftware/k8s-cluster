#!/usr/bin/env python3
from pathlib import Path


def replace_once(source: str, old: str, new: str, *, label: str) -> str:
    count = source.count(old)
    if count != 1:
        raise SystemExit(f'{label}: expected exactly one match, found {count}')
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
main = replace_once(
    main,
    '        assert!(document["components"]["securitySchemes"]["bearer_auth"].is_object());\n',
    '        assert!(document["components"]["securitySchemes"]["bearer_auth"].is_object());\n'
    '        assert!(document["components"]["schemas"]["ErrorResponse"].is_object());\n',
    label='ErrorResponse component test',
)
main_path.write_text(main)

error_path = Path('remote/deployments/dd-embeddings-rs/src/error.rs')
error = error_path.read_text()
error = replace_once(
    error,
    '#[derive(utoipa::IntoResponses)]\npub enum ApiErrorResponses {',
    '#[allow(dead_code)] // Variants are consumed as response metadata by utoipa.\n'
    '#[derive(utoipa::IntoResponses)]\n'
    'pub enum ApiErrorResponses {',
    label='utoipa response metadata dead-code annotation',
)
error_path.write_text(error)

docs_path = Path('remote/deployments/dd-embeddings-rs/src/docs.rs')
docs = docs_path.read_text()
docs = replace_once(
    docs,
    'pub fn finalize(mut openapi: OpenApi) -> OpenApi {',
    '''fn register_schema<T: utoipa::ToSchema>(components: &mut Components) {
    let mut schemas = vec![(
        <T as utoipa::ToSchema>::name().into_owned(),
        <T as utoipa::PartialSchema>::schema(),
    )];
    <T as utoipa::ToSchema>::schemas(&mut schemas);
    components.schemas.extend(schemas);
}

pub fn finalize(mut openapi: OpenApi) -> OpenApi {''',
    label='schema registration helper',
)
docs = replace_once(
    docs,
    '    let components = openapi.components.get_or_insert_with(Components::new);\n'
    '    components.add_security_scheme(',
    '    let components = openapi.components.get_or_insert_with(Components::new);\n'
    '    register_schema::<crate::error::ErrorResponse>(components);\n'
    '    components.add_security_scheme(',
    label='ErrorResponse schema registration',
)
docs_path.write_text(docs)

checker_path = Path('remote/tools/check-openapi-contracts.mjs')
checker = checker_path.read_text()
ref_validation = r'''
function decodePointerToken(token) {
  return token.replaceAll('~1', '/').replaceAll('~0', '~');
}

function resolveLocalRef(document, ref) {
  if (typeof ref !== 'string' || !ref.startsWith('#/')) {
    throw new Error(`external or malformed $ref is not allowed in generated SDK contracts: ${ref}`);
  }
  let current = document;
  for (const rawToken of ref.slice(2).split('/')) {
    const token = decodePointerToken(rawToken);
    if (current === null || typeof current !== 'object' || !(token in current)) {
      return false;
    }
    current = current[token];
  }
  return true;
}

function assertLocalRefsResolve(name, document) {
  const stack = [[document, '$']];
  while (stack.length > 0) {
    const [node, location] = stack.pop();
    if (node === null || typeof node !== 'object') continue;
    if (!Array.isArray(node) && Object.hasOwn(node, '$ref')) {
      const ref = node.$ref;
      if (!resolveLocalRef(document, ref)) {
        throw new Error(`${name}: unresolved local $ref ${ref} at ${location}`);
      }
    }
    for (const [key, value] of Object.entries(node)) {
      stack.push([value, `${location}.${key}`]);
    }
  }
}
'''
checker = replace_once(
    checker,
    '\nfunction validate(name, service, raw) {',
    f'{ref_validation}\nfunction validate(name, service, raw) {{',
    label='local reference validator functions',
)
checker = replace_once(
    checker,
    "  if (!document.components?.securitySchemes?.bearer_auth) {\n"
    "    throw new Error(`${name}: bearer_auth security scheme is missing`);\n"
    "  }\n",
    "  if (!document.components?.securitySchemes?.bearer_auth) {\n"
    "    throw new Error(`${name}: bearer_auth security scheme is missing`);\n"
    "  }\n"
    "  assertLocalRefsResolve(name, document);\n",
    label='local reference validation call',
)
checker_path.write_text(checker)

standard_path = Path('docs/executable-http-api-contract.md')
standard = standard_path.read_text()
standard = replace_once(
    standard,
    '8. all standard documentation routes are present.\n',
    '8. all standard documentation routes are present; and\n'
    '9. every local `$ref` resolves inside the committed document.\n',
    label='document local reference invariant',
)
standard_path.write_text(standard)

print('DEN-445 generator collision, schema closure, checker, and Rust warning fixes applied')
