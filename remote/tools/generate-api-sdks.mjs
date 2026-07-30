#!/usr/bin/env node

import { existsSync } from 'node:fs';
import { mkdir, readdir, readFile, rm, writeFile } from 'node:fs/promises';
import { dirname, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  canonicalJson,
  flatOperations,
  loadSdkInputs,
  prettyJson,
  readRepoFile,
  repoRoot,
  runtimeOperations,
  sdkGeneratorPath,
  sdkOutputRoot,
  sha256,
} from './api-sdk-common.mjs';

const scriptDir = dirname(fileURLToPath(import.meta.url));
const checkMode = process.argv.includes('--check');
const desired = new Map();

function add(path, content) {
  if (!content.endsWith('\n')) {
    content += '\n';
  }
  if (desired.has(path)) {
    throw new Error(`duplicate generated SDK path: ${path}`);
  }
  desired.set(path, content);
}

function q(value) {
  return JSON.stringify(value);
}

function arrayLiteral(values) {
  return `[${values.map(q).join(', ')}]`;
}

function packageNames(language, scope) {
  const suffix = scope === 'public' ? 'public' : 'internal';
  switch (language) {
    case 'typescript':
      return `@oresoftware/k8s-api-sdk-${suffix}`;
    case 'rust':
      return `oresoftware-k8s-api-sdk-${suffix}`;
    case 'dart':
    case 'gleam':
      return `oresoftware_k8s_api_sdk_${suffix}`;
    default:
      throw new Error(`unsupported SDK language ${language}`);
  }
}

function operationForSmoke(operations) {
  return (
    operations.find(
      (operation) =>
        operation.method === 'GET' &&
        operation.path === '/api/docs' &&
        operation.pathParameters.length === 0 &&
        operation.requiredQueryParameters.length === 0 &&
        !operation.requestBodyRequired,
    ) ?? operations.find((operation) => operation.pathParameters.length === 0)
  );
}

function renderTypeScript(scope, catalog) {
  const operations = runtimeOperations(catalog);
  const smoke = operationForSmoke(operations);
  if (!smoke) {
    throw new Error(`${scope}: no operation is suitable for TypeScript smoke testing`);
  }
  const packageName = packageNames('typescript', scope);
  const packageJson = {
    name: packageName,
    version: '0.1.0',
    description: `${scope} fleet HTTP SDK generated from the k8s-cluster OpenAPI contracts`,
    private: scope === 'internal',
    type: 'module',
    sideEffects: false,
    files: ['dist', 'sdk-manifest.json'],
    exports: {
      '.': {
        types: './dist/index.d.ts',
        import: './dist/index.js',
      },
    },
    scripts: {
      build: 'tsc -p tsconfig.json',
      test: 'node --test test/*.test.mjs',
    },
    devDependencies: {
      typescript: '7.0.2',
    },
    engines: {
      node: '>=22.18.0',
    },
    license: 'MIT',
    repository: {
      type: 'git',
      url: 'https://github.com/ORESoftware/k8s-cluster.git',
    },
  };
  const tsconfig = {
    compilerOptions: {
      target: 'ES2024',
      module: 'NodeNext',
      moduleResolution: 'NodeNext',
      lib: ['ES2024', 'DOM', 'DOM.Iterable'],
      strict: true,
      noUncheckedIndexedAccess: true,
      exactOptionalPropertyTypes: true,
      declaration: true,
      sourceMap: true,
      declarationMap: true,
      rootDir: 'src',
      outDir: 'dist',
      verbatimModuleSyntax: true,
      skipLibCheck: false,
      forceConsistentCasingInFileNames: true,
    },
    include: ['src/**/*.ts'],
  };
  const source = `export type SdkScope = 'public' | 'internal';
export type ParameterScalar = string | number | boolean;
export type QueryValue = ParameterScalar | readonly ParameterScalar[] | null | undefined;

export interface ApiOperation {
  readonly service: string;
  readonly operationId: string;
  readonly method: string;
  readonly path: string;
  readonly pathParameters: readonly string[];
  readonly requiredQueryParameters: readonly string[];
  readonly optionalQueryParameters: readonly string[];
  readonly requestBodyRequired: boolean;
  readonly contractSha256: string;
}

export interface BuildRequestOptions {
  readonly baseUrl: string;
  readonly operationId: string;
  readonly pathParameters?: Readonly<Record<string, ParameterScalar>>;
  readonly queryParameters?: Readonly<Record<string, QueryValue>>;
  readonly headers?: HeadersInit;
  readonly body?: BodyInit | Readonly<Record<string, unknown>> | readonly unknown[] | null;
}

export interface CallOptions extends Omit<BuildRequestOptions, 'baseUrl' | 'operationId'> {}

export interface ApiClientOptions {
  readonly baseUrls: Readonly<Record<string, string>>;
  readonly headers?: HeadersInit;
  readonly fetch?: typeof fetch;
}

export const SDK_SCOPE: SdkScope = ${q(scope)};
export const CATALOG_SHA256 = ${q(catalog.catalogSha256)};
export const OPERATION_COUNT = ${operations.length};
export const OPERATIONS: readonly ApiOperation[] = Object.freeze(${JSON.stringify(operations, null, 2)});

const operationsById = new Map(OPERATIONS.map((operation) => [operation.operationId, operation]));

export class ApiSdkError extends Error {
  override readonly name = 'ApiSdkError';
}

export function operationById(operationId: string): ApiOperation {
  const operation = operationsById.get(operationId);
  if (!operation) {
    throw new ApiSdkError(\`Unknown operationId: \${operationId}\`);
  }
  return operation;
}

function assertKnownParameters(
  kind: 'path' | 'query',
  values: Readonly<Record<string, unknown>>,
  allowed: readonly string[],
): void {
  const allowedSet = new Set(allowed);
  for (const name of Object.keys(values)) {
    if (!allowedSet.has(name)) {
      throw new ApiSdkError(\`Unknown \${kind} parameter \${name}\`);
    }
  }
}

function nativeBody(value: unknown): value is BodyInit {
  return (
    typeof value === 'string' ||
    value instanceof ArrayBuffer ||
    ArrayBuffer.isView(value) ||
    (typeof Blob !== 'undefined' && value instanceof Blob) ||
    (typeof FormData !== 'undefined' && value instanceof FormData) ||
    (typeof URLSearchParams !== 'undefined' && value instanceof URLSearchParams) ||
    (typeof ReadableStream !== 'undefined' && value instanceof ReadableStream)
  );
}

export function buildRequest(options: BuildRequestOptions): Request {
  const operation = operationById(options.operationId);
  const pathParameters = options.pathParameters ?? {};
  const queryParameters = options.queryParameters ?? {};
  assertKnownParameters('path', pathParameters, operation.pathParameters);
  assertKnownParameters(
    'query',
    queryParameters,
    [...operation.requiredQueryParameters, ...operation.optionalQueryParameters],
  );

  let path = operation.path;
  for (const name of operation.pathParameters) {
    const value = pathParameters[name];
    if (value === undefined) {
      throw new ApiSdkError(\`Missing path parameter \${name}\`);
    }
    path = path.replaceAll(\`{\${name}}\`, encodeURIComponent(String(value)));
  }
  for (const name of operation.requiredQueryParameters) {
    if (queryParameters[name] === undefined || queryParameters[name] === null) {
      throw new ApiSdkError(\`Missing query parameter \${name}\`);
    }
  }

  const url = new URL(options.baseUrl.replace(/\\/+$/, '') + path);
  for (const [name, raw] of Object.entries(queryParameters)) {
    if (raw === undefined || raw === null) continue;
    const values = Array.isArray(raw) ? raw : [raw];
    for (const value of values) {
      url.searchParams.append(name, String(value));
    }
  }

  if (operation.requestBodyRequired && (options.body === undefined || options.body === null)) {
    throw new ApiSdkError(\`Operation \${operation.operationId} requires a request body\`);
  }
  if ((operation.method === 'GET' || operation.method === 'HEAD') && options.body != null) {
    throw new ApiSdkError(\`Operation \${operation.operationId} does not permit a request body\`);
  }

  const headers = new Headers(options.headers);
  let body: BodyInit | null | undefined;
  if (options.body === undefined || options.body === null || nativeBody(options.body)) {
    body = options.body;
  } else {
    body = JSON.stringify(options.body);
    if (!headers.has('content-type')) headers.set('content-type', 'application/json');
  }
  const init: RequestInit = { method: operation.method, headers };
  if (body !== undefined && body !== null) init.body = body;
  return new Request(url, init);
}

export class ApiClient {
  readonly #baseUrls: Readonly<Record<string, string>>;
  readonly #headers: Headers;
  readonly #fetch: typeof fetch;

  constructor(options: ApiClientOptions) {
    this.#baseUrls = options.baseUrls;
    this.#headers = new Headers(options.headers);
    this.#fetch = options.fetch ?? globalThis.fetch;
    if (typeof this.#fetch !== 'function') {
      throw new ApiSdkError('No fetch implementation is available');
    }
  }

  async call(operationId: string, options: CallOptions = {}): Promise<Response> {
    const operation = operationById(operationId);
    const baseUrl = this.#baseUrls[operation.service];
    if (!baseUrl) {
      throw new ApiSdkError(\`Missing base URL for service \${operation.service}\`);
    }
    const headers = new Headers(this.#headers);
    new Headers(options.headers).forEach((value, name) => headers.set(name, value));
    const request = buildRequest({ ...options, operationId, baseUrl, headers });
    return this.#fetch(request);
  }
}
`;
  const test = `import assert from 'node:assert/strict';
import test from 'node:test';
import { CATALOG_SHA256, OPERATIONS, SDK_SCOPE, buildRequest } from '../dist/index.js';

test('generated ${scope} fleet SDK builds a canonical docs request', () => {
  assert.equal(SDK_SCOPE, ${q(scope)});
  assert.equal(CATALOG_SHA256, ${q(catalog.catalogSha256)});
  assert.equal(OPERATIONS.length, ${operations.length});
  const request = buildRequest({
    baseUrl: 'https://example.test/',
    operationId: ${q(smoke.operationId)},
  });
  assert.equal(request.method, ${q(smoke.method)});
  assert.equal(request.url, 'https://example.test${smoke.path}');
});
`;
  return {
    packageName,
    files: {
      'package.json': prettyJson(packageJson),
      'tsconfig.json': prettyJson(tsconfig),
      'src/index.ts': source,
      'test/smoke.test.mjs': test,
    },
  };
}

function rustSlice(values) {
  return `&[${values.map(q).join(', ')}]`;
}

function renderRust(scope, catalog) {
  const operations = runtimeOperations(catalog);
  const smoke = operationForSmoke(operations);
  if (!smoke) {
    throw new Error(`${scope}: no operation is suitable for Rust smoke testing`);
  }
  const packageName = packageNames('rust', scope);
  const operationRows = operations
    .map(
      (operation) => `    ApiOperation {
        service: ${q(operation.service)},
        operation_id: ${q(operation.operationId)},
        method: ${q(operation.method)},
        path: ${q(operation.path)},
        path_parameters: ${rustSlice(operation.pathParameters)},
        required_query_parameters: ${rustSlice(operation.requiredQueryParameters)},
        optional_query_parameters: ${rustSlice(operation.optionalQueryParameters)},
        request_body_required: ${operation.requestBodyRequired},
        contract_sha256: ${q(operation.contractSha256)},
    },`,
    )
    .join('\n');
  const source = `use std::error::Error;
use std::fmt::{Display, Formatter};

pub const SDK_SCOPE: &str = ${q(scope)};
pub const CATALOG_SHA256: &str = ${q(catalog.catalogSha256)};
pub const OPERATION_COUNT: usize = ${operations.length};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiOperation {
    pub service: &'static str,
    pub operation_id: &'static str,
    pub method: &'static str,
    pub path: &'static str,
    pub path_parameters: &'static [&'static str],
    pub required_query_parameters: &'static [&'static str],
    pub optional_query_parameters: &'static [&'static str],
    pub request_body_required: bool,
    pub contract_sha256: &'static str,
}

pub static OPERATIONS: &[ApiOperation] = &[
${operationRows}
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestSpec {
    pub service: &'static str,
    pub operation_id: &'static str,
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildError {
    UnknownOperation(String),
    MissingPathParameter(String),
    MissingQueryParameter(String),
    UnknownPathParameter(String),
    UnknownQueryParameter(String),
    MissingRequestBody(String),
    BodyNotAllowed(String),
}

impl Display for BuildError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownOperation(value) => write!(formatter, "unknown operationId: {value}"),
            Self::MissingPathParameter(value) => write!(formatter, "missing path parameter: {value}"),
            Self::MissingQueryParameter(value) => write!(formatter, "missing query parameter: {value}"),
            Self::UnknownPathParameter(value) => write!(formatter, "unknown path parameter: {value}"),
            Self::UnknownQueryParameter(value) => write!(formatter, "unknown query parameter: {value}"),
            Self::MissingRequestBody(value) => write!(formatter, "operation requires a body: {value}"),
            Self::BodyNotAllowed(value) => write!(formatter, "operation does not permit a body: {value}"),
        }
    }
}

impl Error for BuildError {}

pub fn operation_by_id(operation_id: &str) -> Option<&'static ApiOperation> {
    OPERATIONS.iter().find(|operation| operation.operation_id == operation_id)
}

fn value_for<'a>(pairs: &'a [(&str, &str)], name: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find_map(|(key, value)| (*key == name).then_some(*value))
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

pub fn build_request(
    base_url: &str,
    operation_id: &str,
    path_parameters: &[(&str, &str)],
    query_parameters: &[(&str, &str)],
    headers: &[(&str, &str)],
    body: Option<String>,
) -> Result<RequestSpec, BuildError> {
    let operation = operation_by_id(operation_id)
        .ok_or_else(|| BuildError::UnknownOperation(operation_id.to_owned()))?;

    for &(name, _) in path_parameters {
        if !operation.path_parameters.contains(&name) {
            return Err(BuildError::UnknownPathParameter(name.to_owned()));
        }
    }
    let allowed_query = operation
        .required_query_parameters
        .iter()
        .chain(operation.optional_query_parameters.iter());
    for &(name, _) in query_parameters {
        if !allowed_query.clone().any(|candidate| *candidate == name) {
            return Err(BuildError::UnknownQueryParameter(name.to_owned()));
        }
    }

    let mut path = operation.path.to_owned();
    for name in operation.path_parameters {
        let value = value_for(path_parameters, name)
            .ok_or_else(|| BuildError::MissingPathParameter((*name).to_owned()))?;
        path = path.replace(&format!("{{{name}}}"), &percent_encode(value));
    }
    for name in operation.required_query_parameters {
        if value_for(query_parameters, name).is_none() {
            return Err(BuildError::MissingQueryParameter((*name).to_owned()));
        }
    }

    if operation.request_body_required && body.is_none() {
        return Err(BuildError::MissingRequestBody(operation_id.to_owned()));
    }
    if matches!(operation.method, "GET" | "HEAD") && body.is_some() {
        return Err(BuildError::BodyNotAllowed(operation_id.to_owned()));
    }

    let mut url = format!("{}{}", base_url.trim_end_matches('/'), path);
    if !query_parameters.is_empty() {
        let query = query_parameters
            .iter()
            .map(|(name, value)| format!("{}={}", percent_encode(name), percent_encode(value)))
            .collect::<Vec<_>>()
            .join("&");
        url.push('?');
        url.push_str(&query);
    }

    Ok(RequestSpec {
        service: operation.service,
        operation_id: operation.operation_id,
        method: operation.method,
        url,
        headers: headers
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect(),
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_canonical_docs_request() {
        assert_eq!(SDK_SCOPE, ${q(scope)});
        assert_eq!(CATALOG_SHA256, ${q(catalog.catalogSha256)});
        assert_eq!(OPERATIONS.len(), ${operations.length});
        let request = build_request(
            "https://example.test/",
            ${q(smoke.operationId)},
            &[],
            &[],
            &[],
            None,
        )
        .expect("docs request should build");
        assert_eq!(request.method, ${q(smoke.method)});
        assert_eq!(request.url, ${q(`https://example.test${smoke.path}`)});
    }
}
`;
  const cargo = `[package]
name = ${q(packageName)}
version = "0.1.0"
edition = "2024"
rust-version = "1.85"
description = ${q(`${scope} fleet HTTP SDK generated from k8s-cluster OpenAPI contracts`)}
license = "MIT"
repository = "https://github.com/ORESoftware/k8s-cluster"
publish = false

[lib]
path = "src/lib.rs"
`;
  return {
    packageName,
    files: {
      'Cargo.toml': cargo,
      'src/lib.rs': source,
    },
  };
}

function renderDart(scope, catalog) {
  const operations = runtimeOperations(catalog);
  const smoke = operationForSmoke(operations);
  if (!smoke) {
    throw new Error(`${scope}: no operation is suitable for Dart smoke testing`);
  }
  const packageName = packageNames('dart', scope);
  const rows = operations
    .map(
      (operation) => `  ApiOperation(
    service: ${q(operation.service)},
    operationId: ${q(operation.operationId)},
    method: ${q(operation.method)},
    path: ${q(operation.path)},
    pathParameters: ${arrayLiteral(operation.pathParameters)},
    requiredQueryParameters: ${arrayLiteral(operation.requiredQueryParameters)},
    optionalQueryParameters: ${arrayLiteral(operation.optionalQueryParameters)},
    requestBodyRequired: ${operation.requestBodyRequired},
    contractSha256: ${q(operation.contractSha256)},
  ),`,
    )
    .join('\n');
  const source = `const String sdkScope = ${q(scope)};
const String catalogSha256 = ${q(catalog.catalogSha256)};
const int operationCount = ${operations.length};

class ApiOperation {
  const ApiOperation({
    required this.service,
    required this.operationId,
    required this.method,
    required this.path,
    required this.pathParameters,
    required this.requiredQueryParameters,
    required this.optionalQueryParameters,
    required this.requestBodyRequired,
    required this.contractSha256,
  });

  final String service;
  final String operationId;
  final String method;
  final String path;
  final List<String> pathParameters;
  final List<String> requiredQueryParameters;
  final List<String> optionalQueryParameters;
  final bool requestBodyRequired;
  final String contractSha256;
}

class ApiRequest {
  const ApiRequest({
    required this.service,
    required this.operationId,
    required this.method,
    required this.url,
    required this.headers,
    this.body,
  });

  final String service;
  final String operationId;
  final String method;
  final String url;
  final Map<String, String> headers;
  final Object? body;
}

class ApiSdkException implements Exception {
  const ApiSdkException(this.message);
  final String message;

  @override
  String toString() => 'ApiSdkException: $message';
}

const List<ApiOperation> operations = <ApiOperation>[
${rows}
];

final Map<String, ApiOperation> _operationsById = <String, ApiOperation>{
  for (final ApiOperation operation in operations) operation.operationId: operation,
};

ApiOperation operationById(String operationId) {
  final ApiOperation? operation = _operationsById[operationId];
  if (operation == null) {
    throw ApiSdkException('Unknown operationId: $operationId');
  }
  return operation;
}

ApiRequest buildRequest({
  required String baseUrl,
  required String operationId,
  Map<String, String> pathParameters = const <String, String>{},
  Map<String, String> queryParameters = const <String, String>{},
  Map<String, String> headers = const <String, String>{},
  Object? body,
}) {
  final ApiOperation operation = operationById(operationId);
  final Set<String> allowedPath = operation.pathParameters.toSet();
  final Set<String> allowedQuery = <String>{
    ...operation.requiredQueryParameters,
    ...operation.optionalQueryParameters,
  };
  for (final String name in pathParameters.keys) {
    if (!allowedPath.contains(name)) {
      throw ApiSdkException('Unknown path parameter $name');
    }
  }
  for (final String name in queryParameters.keys) {
    if (!allowedQuery.contains(name)) {
      throw ApiSdkException('Unknown query parameter $name');
    }
  }

  String path = operation.path;
  for (final String name in operation.pathParameters) {
    final String? value = pathParameters[name];
    if (value == null) {
      throw ApiSdkException('Missing path parameter $name');
    }
    path = path.replaceAll('{$name}', Uri.encodeComponent(value));
  }
  for (final String name in operation.requiredQueryParameters) {
    if (!queryParameters.containsKey(name)) {
      throw ApiSdkException('Missing query parameter $name');
    }
  }
  if (operation.requestBodyRequired && body == null) {
    throw ApiSdkException('Operation $operationId requires a request body');
  }
  if ((operation.method == 'GET' || operation.method == 'HEAD') && body != null) {
    throw ApiSdkException('Operation $operationId does not permit a request body');
  }

  final StringBuffer url = StringBuffer(baseUrl.replaceFirst(RegExp(r'/+$'), ''))..write(path);
  if (queryParameters.isNotEmpty) {
    url
      ..write('?')
      ..write(
        queryParameters.entries
            .map(
              (MapEntry<String, String> entry) =>
                  '\${Uri.encodeQueryComponent(entry.key)}=\${Uri.encodeQueryComponent(entry.value)}',
            )
            .join('&'),
      );
  }
  return ApiRequest(
    service: operation.service,
    operationId: operation.operationId,
    method: operation.method,
    url: url.toString(),
    headers: Map<String, String>.unmodifiable(headers),
    body: body,
  );
}
`;
  const smokeSource = `import 'package:${packageName}/dd_api_sdk.dart';

void main() {
  if (sdkScope != ${q(scope)}) throw StateError('scope drift');
  if (catalogSha256 != ${q(catalog.catalogSha256)}) throw StateError('catalog drift');
  if (operations.length != ${operations.length}) throw StateError('operation count drift');
  final ApiRequest request = buildRequest(
    baseUrl: 'https://example.test/',
    operationId: ${q(smoke.operationId)},
  );
  if (request.method != ${q(smoke.method)}) throw StateError('method drift');
  if (request.url != ${q(`https://example.test${smoke.path}`)}) throw StateError('URL drift');
}
`;
  const pubspec = `name: ${packageName}
version: 0.1.0
description: ${scope} fleet HTTP SDK generated from k8s-cluster OpenAPI contracts
repository: https://github.com/ORESoftware/k8s-cluster
${scope === 'internal' ? 'publish_to: none\n' : ''}environment:
  sdk: ">=3.4.0 <4.0.0"
`;
  const analysisOptions = `analyzer:
  language:
    strict-casts: true
    strict-inference: true
    strict-raw-types: true
linter:
  rules:
    - always_declare_return_types
    - avoid_dynamic_calls
    - prefer_final_locals
`;
  return {
    packageName,
    files: {
      'pubspec.yaml': pubspec,
      'analysis_options.yaml': analysisOptions,
      'lib/dd_api_sdk.dart': source,
      'bin/smoke.dart': smokeSource,
    },
  };
}

function gleamList(values) {
  return `[${values.map(q).join(', ')}]`;
}

function renderGleam(scope, catalog) {
  const operations = runtimeOperations(catalog);
  const smoke = operationForSmoke(operations);
  if (!smoke) {
    throw new Error(`${scope}: no operation is suitable for Gleam smoke testing`);
  }
  const packageName = packageNames('gleam', scope);
  const rows = operations
    .map(
      (operation) => `    ApiOperation(
      service: ${q(operation.service)},
      operation_id: ${q(operation.operationId)},
      method: ${q(operation.method)},
      path: ${q(operation.path)},
      path_parameters: ${gleamList(operation.pathParameters)},
      required_query_parameters: ${gleamList(operation.requiredQueryParameters)},
      optional_query_parameters: ${gleamList(operation.optionalQueryParameters)},
      request_body_required: ${operation.requestBodyRequired ? 'True' : 'False'},
      contract_sha256: ${q(operation.contractSha256)},
    ),`,
    )
    .join('\n');
  const source = `import gleam/list
import gleam/option.{type Option, None, Some}
import gleam/string
import gleam/uri

pub const sdk_scope = ${q(scope)}
pub const catalog_sha256 = ${q(catalog.catalogSha256)}
pub const operation_count = ${operations.length}

pub type ApiOperation {
  ApiOperation(
    service: String,
    operation_id: String,
    method: String,
    path: String,
    path_parameters: List(String),
    required_query_parameters: List(String),
    optional_query_parameters: List(String),
    request_body_required: Bool,
    contract_sha256: String,
  )
}

pub type ApiRequest {
  ApiRequest(
    service: String,
    operation_id: String,
    method: String,
    url: String,
    headers: List(#(String, String)),
    body: Option(String),
  )
}

pub type BuildError {
  UnknownOperation(String)
  MissingPathParameter(String)
  MissingQueryParameter(String)
  UnknownPathParameter(String)
  UnknownQueryParameter(String)
  MissingRequestBody(String)
  BodyNotAllowed(String)
}

pub fn operations() -> List(ApiOperation) {
  [
${rows}
  ]
}

fn find_operation_in(items: List(ApiOperation), operation_id: String) -> Result(ApiOperation, BuildError) {
  case items {
    [] -> Error(UnknownOperation(operation_id))
    [first, ..rest] ->
      case first.operation_id == operation_id {
        True -> Ok(first)
        False -> find_operation_in(rest, operation_id)
      }
  }
}

pub fn operation_by_id(operation_id: String) -> Result(ApiOperation, BuildError) {
  find_operation_in(operations(), operation_id)
}

fn name_in(names: List(String), target: String) -> Bool {
  case names {
    [] -> False
    [first, ..rest] ->
      case first == target {
        True -> True
        False -> name_in(rest, target)
      }
  }
}

fn value_for(parameters: List(#(String, String)), target: String) -> Result(String, Nil) {
  case parameters {
    [] -> Error(Nil)
    [#(name, value), ..rest] ->
      case name == target {
        True -> Ok(value)
        False -> value_for(rest, target)
      }
  }
}

fn validate_path_keys(parameters: List(#(String, String)), allowed: List(String)) -> Result(Nil, BuildError) {
  case parameters {
    [] -> Ok(Nil)
    [#(name, _), ..rest] ->
      case name_in(allowed, name) {
        True -> validate_path_keys(rest, allowed)
        False -> Error(UnknownPathParameter(name))
      }
  }
}

fn validate_query_keys(parameters: List(#(String, String)), allowed: List(String)) -> Result(Nil, BuildError) {
  case parameters {
    [] -> Ok(Nil)
    [#(name, _), ..rest] ->
      case name_in(allowed, name) {
        True -> validate_query_keys(rest, allowed)
        False -> Error(UnknownQueryParameter(name))
      }
  }
}

fn apply_path_parameters(
  path: String,
  names: List(String),
  parameters: List(#(String, String)),
) -> Result(String, BuildError) {
  case names {
    [] -> Ok(path)
    [name, ..rest] ->
      case value_for(parameters, name) {
        Error(_) -> Error(MissingPathParameter(name))
        Ok(value) ->
          apply_path_parameters(
            string.replace(path, "{" <> name <> "}", uri.percent_encode(value)),
            rest,
            parameters,
          )
      }
  }
}

fn ensure_required_query(
  names: List(String),
  parameters: List(#(String, String)),
) -> Result(Nil, BuildError) {
  case names {
    [] -> Ok(Nil)
    [name, ..rest] ->
      case value_for(parameters, name) {
        Error(_) -> Error(MissingQueryParameter(name))
        Ok(_) -> ensure_required_query(rest, parameters)
      }
  }
}

fn validate_body(operation: ApiOperation, body: Option(String)) -> Result(Nil, BuildError) {
  case operation.request_body_required, body {
    True, None -> Error(MissingRequestBody(operation.operation_id))
    _, Some(_) if operation.method == "GET" || operation.method == "HEAD" ->
      Error(BodyNotAllowed(operation.operation_id))
    _, _ -> Ok(Nil)
  }
}

fn trim_base_url(base_url: String) -> String {
  case string.ends_with(base_url, "/") {
    True -> string.drop_end(base_url, 1)
    False -> base_url
  }
}

pub fn build_request(
  base_url: String,
  operation_id: String,
  path_parameters: List(#(String, String)),
  query_parameters: List(#(String, String)),
  headers: List(#(String, String)),
  body: Option(String),
) -> Result(ApiRequest, BuildError) {
  case operation_by_id(operation_id) {
    Error(error) -> Error(error)
    Ok(operation) -> {
      let allowed_query =
        list.append(operation.required_query_parameters, operation.optional_query_parameters)
      case
        validate_path_keys(path_parameters, operation.path_parameters),
        validate_query_keys(query_parameters, allowed_query),
        apply_path_parameters(operation.path, operation.path_parameters, path_parameters),
        ensure_required_query(operation.required_query_parameters, query_parameters),
        validate_body(operation, body)
      {
        Error(error), _, _, _, _ -> Error(error)
        _, Error(error), _, _, _ -> Error(error)
        _, _, Error(error), _, _ -> Error(error)
        _, _, _, Error(error), _ -> Error(error)
        _, _, _, _, Error(error) -> Error(error)
        Ok(Nil), Ok(Nil), Ok(path), Ok(Nil), Ok(Nil) -> {
          let query = uri.query_to_string(query_parameters)
          let url =
            case query {
              "" -> trim_base_url(base_url) <> path
              value -> trim_base_url(base_url) <> path <> "?" <> value
            }
          Ok(ApiRequest(
            service: operation.service,
            operation_id: operation.operation_id,
            method: operation.method,
            url: url,
            headers: headers,
            body: body,
          ))
        }
      }
    }
  }
}
`;
  const test = `import dd_api_sdk
import gleam/option.{None}
import gleeunit

pub fn main() {
  gleeunit.main()
}

pub fn builds_canonical_docs_request_test() {
  assert dd_api_sdk.sdk_scope == ${q(scope)}
  assert dd_api_sdk.catalog_sha256 == ${q(catalog.catalogSha256)} // gitleaks:allow
  assert list.length(dd_api_sdk.operations()) == ${operations.length}
  let assert Ok(request) = dd_api_sdk.build_request(
    "https://example.test/",
    ${q(smoke.operationId)},
    [],
    [],
    [],
    None,
  )
  assert request.method == ${q(smoke.method)}
  assert request.url == ${q(`https://example.test${smoke.path}`)}
}
`;
  const fixedTest = test.replace('import gleam/option.{None}\n', 'import gleam/list\nimport gleam/option.{None}\n');
  const gleamToml = `name = ${q(packageName)}
version = "0.1.0"
description = ${q(`${scope} fleet HTTP SDK generated from k8s-cluster OpenAPI contracts`)}
licences = ["MIT"]
repository = { type = "github", user = "ORESoftware", repo = "k8s-cluster" }
target = "erlang"

[dependencies]
gleam_stdlib = ">= 1.0.3 and < 2.0.0"

[dev-dependencies]
gleeunit = ">= 1.11.0 and < 2.0.0"
`;
  return {
    packageName,
    files: {
      'gleam.toml': gleamToml,
      'src/dd_api_sdk.gleam': source,
      [`test/${packageName}_test.gleam`]: fixedTest,
    },
  };
}

async function listFiles(root, prefix = '') {
  if (!existsSync(root)) {
    return [];
  }
  const files = [];
  for (const entry of await readdir(root, { withFileTypes: true })) {
    const relativePath = prefix ? `${prefix}/${entry.name}` : entry.name;
    const absolutePath = resolve(root, entry.name);
    if (entry.isDirectory()) {
      files.push(...(await listFiles(absolutePath, relativePath)));
    } else if (entry.isFile()) {
      files.push(relativePath);
    }
  }
  return files.sort();
}

async function writeOrCheck() {
  const currentFiles = await listFiles(sdkOutputRoot);
  const desiredFiles = [...desired.keys()].sort();
  if (checkMode) {
    const failures = [];
    const allFiles = [...new Set([...currentFiles, ...desiredFiles])].sort();
    for (const path of allFiles) {
      if (!desired.has(path)) {
        failures.push(`stale generated SDK file: remote/api-sdks/${path}`);
        continue;
      }
      const absolutePath = resolve(sdkOutputRoot, path);
      if (!existsSync(absolutePath)) {
        failures.push(`missing generated SDK file: remote/api-sdks/${path}`);
        continue;
      }
      const current = await readFile(absolutePath, 'utf8');
      if (current !== desired.get(path)) {
        failures.push(`outdated generated SDK file: remote/api-sdks/${path}`);
      }
    }
    if (failures.length > 0) {
      throw new Error(failures.join('\n'));
    }
    console.log(`verified ${desiredFiles.length} generated SDK file(s)`);
    return;
  }

  await rm(sdkOutputRoot, { recursive: true, force: true });
  for (const path of desiredFiles) {
    const absolutePath = resolve(sdkOutputRoot, path);
    await mkdir(dirname(absolutePath), { recursive: true });
    await writeFile(absolutePath, desired.get(path));
  }
  console.log(`generated ${desiredFiles.length} SDK file(s)`);
}

async function main() {
  const inputs = await loadSdkInputs();
  const generatorRaw = await readRepoFile(sdkGeneratorPath);
  const catalogFileDigests = {};
  for (const scope of ['public', 'internal']) {
    const content = prettyJson(inputs.catalogs[scope]);
    add(`contracts/${scope}.json`, content);
    catalogFileDigests[scope] = sha256(content);
  }

  const packageEntries = [];
  for (const scope of ['public', 'internal']) {
    const catalog = inputs.catalogs[scope];
    for (const [language, renderer] of [
      ['typescript', renderTypeScript],
      ['rust', renderRust],
      ['dart', renderDart],
      ['gleam', renderGleam],
    ]) {
      const rendered = renderer(scope, catalog);
      const packageRoot = `${language}/${scope}`;
      const generatedFiles = Object.entries(rendered.files)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([path, content]) => {
          add(`${packageRoot}/${path}`, content);
          return { path, sha256: sha256(content.endsWith('\n') ? content : `${content}\n`) };
        });
      const manifest = {
        schemaVersion: 1,
        language,
        scope,
        packageName: rendered.packageName,
        generatedBy: sdkGeneratorPath,
        catalogPath: `remote/api-sdks/contracts/${scope}.json`,
        catalogSha256: catalog.catalogSha256,
        catalogFileSha256: catalogFileDigests[scope],
        serviceCount: catalog.serviceCount,
        operationCount: catalog.operationCount,
        skippedServices: catalog.skippedServices,
        generatedFiles,
      };
      const manifestContent = prettyJson(manifest);
      add(`${packageRoot}/sdk-manifest.json`, manifestContent);
      packageEntries.push({
        language,
        scope,
        packageName: rendered.packageName,
        path: `remote/api-sdks/${packageRoot}`,
        manifestPath: `remote/api-sdks/${packageRoot}/sdk-manifest.json`,
        manifestSha256: sha256(manifestContent),
        catalogSha256: catalog.catalogSha256,
        operationCount: catalog.operationCount,
      });
    }
  }

  const lock = {
    schemaVersion: 1,
    generatedBy: sdkGeneratorPath,
    generatorSha256: sha256(generatorRaw),
    indexPath: 'remote/deployments/generated-api-docs-index.json',
    indexSha256: inputs.indexSha256,
    scopes: Object.fromEntries(
      ['public', 'internal'].map((scope) => {
        const catalog = inputs.catalogs[scope];
        return [
          scope,
          {
            catalogPath: `remote/api-sdks/contracts/${scope}.json`,
            catalogSha256: catalog.catalogSha256,
            catalogFileSha256: catalogFileDigests[scope],
            serviceCount: catalog.serviceCount,
            operationCount: catalog.operationCount,
            skippedServices: catalog.skippedServices,
            specs: catalog.services.map((service) => ({
              service: service.service,
              specPath: service.specPath,
              specSha256: service.specSha256,
              operationCount: service.operationCount,
            })),
          },
        ];
      }),
    ),
    packages: packageEntries.sort((left, right) => {
      return `${left.language}:${left.scope}`.localeCompare(`${right.language}:${right.scope}`);
    }),
  };
  add('sdk-lock.json', prettyJson(lock));

  const skippedRows = inputs.skippedServices.length
    ? inputs.skippedServices
        .map(
          (service) =>
            `| \`${service.service}\` | \`${service.language}\` | \`${service.sourceRepository}\` | ${service.reason} |`,
        )
        .join('\n')
    : '| _none_ | | | |';
  add(
    'README.md',
    `# Fleet API SDKs

These packages are generated from the exact OpenAPI 3.1 artifacts indexed by
\`remote/deployments/generated-api-docs-index.json\`. Never edit generated package files by hand.

## Packages

Eight packages are produced: public and internal variants for TypeScript, Rust, Dart, and Gleam.
Public packages contain only the fail-closed runtime contract. Internal packages contain every
available operation and are intended for trusted service-to-service callers.

All request builders reject unknown parameters, require declared path/query values, enforce request
body presence, and percent-encode path and query values. The root \`sdk-lock.json\` records the
SHA-256 digest of every source OpenAPI document, both scope catalogs, every package manifest, and the
generator itself.

Current generated coverage:

- public: ${inputs.catalogs.public.serviceCount} services / ${inputs.catalogs.public.operationCount} operations
- internal: ${inputs.catalogs.internal.serviceCount} services / ${inputs.catalogs.internal.operationCount} operations
- temporarily unavailable deployment gitlinks: ${inputs.skippedServices.length}

## Temporary gitlink exclusions

The generator fails for missing normal deployment artifacts. It may skip only an uninitialized Git
gitlink, and the exact upstream repository is recorded below. These services must be migrated in their
source repositories and then their parent gitlinks must be bumped.

| Service | Language | Source repository | Reason |
|---|---|---|---|
${skippedRows}

## Commands

\`\`\`bash
node remote/tools/generate-api-sdks.mjs
node remote/tools/generate-api-sdks.mjs --check
node remote/tools/validate-api-sdks.mjs
\`\`\`

The generated SDKs currently provide strongly synchronized operation catalogs and dependency-light
request builders. Request and response models become richer automatically as each server migrates
from the compatibility scanner to its native typed OpenAPI adapter.
`,
  );

  await writeOrCheck();
}

main().catch((error) => {
  console.error(error instanceof Error ? error.stack : error);
  process.exitCode = 1;
});
