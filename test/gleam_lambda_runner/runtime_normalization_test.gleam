//// Tests for `lambda_child_runner`'s pure definition-parsing layer, driven
//// through the exported FFI surface (`invoke` / `invoke_definition`): legacy
//// runtime aliases normalize onto canonical pools (javascript/typescript ->
//// nodejs, python -> python3, playwright/puppeteer/scraper -> browser, ...),
//// identifiers are validated before any database work, and pool-dispatch /
//// container config fields are rejected before anything is spawned or
//// published. Every case here exercises an error branch that returns before
//// a child process, psql, or NATS connection is touched, so the tests run
//// hermetically.

import gleam/string
import gleam_lambda_runner/child_process
import gleeunit/should

/// A host-execution (containerized=false) definition with the given runtime
/// value plus optional extra JSON fields (each prefixed with a comma).
fn host_definition(runtime: String, extra: String) -> String {
  "{"
  <> "\"id\":\"00000000-0000-0000-0000-0000000000aa\","
  <> "\"slug\":\"norm-check\","
  <> "\"functionBody\":\"return { status: 200 };\","
  <> "\"runtime\":\""
  <> runtime
  <> "\","
  <> "\"status\":\"active\","
  <> "\"containerized\":false"
  <> extra
  <> "}"
}

fn invoke_runtime(runtime: String) -> Result(String, String) {
  child_process.invoke_definition(
    "",
    "norm-check",
    host_definition(runtime, ""),
    "{}",
    1000,
    1000,
  )
}

fn expect_error_containing(result: Result(String, String), needle: String) {
  should.be_error(result)
  case result {
    Error(message) ->
      string.contains(message, needle)
      |> should.be_true
    Ok(_) -> should.fail()
  }
}

// --- Legacy runtime aliases normalize onto canonical pools ----------------
//
// Only `nodejs` is host-allowed by default (LAMBDA_ALLOW_HOST_RUNTIMES), so a
// non-containerized definition using any other pool fails fast with an error
// naming the *canonical* runtime — which is exactly the normalization proof:
// the legacy value went in, the canonical pool name comes out.

pub fn legacy_python_normalizes_to_python3_pool_test() {
  expect_error_containing(
    invoke_runtime("python"),
    "containerized=true for host execution: python3",
  )
}

pub fn legacy_shell_normalizes_to_bash_pool_test() {
  expect_error_containing(
    invoke_runtime("shell"),
    "containerized=true for host execution: bash",
  )
}

pub fn legacy_go_normalizes_to_golang_pool_test() {
  expect_error_containing(
    invoke_runtime("go"),
    "containerized=true for host execution: golang",
  )
}

pub fn legacy_erl_and_ex_normalize_to_beam_pools_test() {
  expect_error_containing(
    invoke_runtime("erl"),
    "containerized=true for host execution: erlang",
  )
  expect_error_containing(
    invoke_runtime("ex"),
    "containerized=true for host execution: elixir",
  )
}

pub fn rust_and_gleam_aliases_normalize_to_canonical_pools_test() {
  expect_error_containing(
    invoke_runtime("rs"),
    "containerized=true for host execution: rust",
  )
  expect_error_containing(
    invoke_runtime("gleamlang"),
    "containerized=true for host execution: gleam",
  )
}

pub fn playwright_normalizes_to_browser_pool_test() {
  expect_error_containing(
    invoke_runtime("playwright"),
    "containerized=true for host execution: browser",
  )
}

pub fn puppeteer_and_scraper_normalize_to_browser_pool_test() {
  expect_error_containing(
    invoke_runtime("puppeteer"),
    "containerized=true for host execution: browser",
  )
  expect_error_containing(
    invoke_runtime("scraper"),
    "containerized=true for host execution: browser",
  )
}

// `javascript`/`typescript` normalize to `nodejs`, which IS host-allowed by
// default — so the definition sails past the runtime gate and fails on the
// next validation instead (an unsafe reuseKey). Raw "javascript" is not a
// supported runtime, so reaching the reuseKey check proves normalization.

pub fn legacy_javascript_normalizes_to_nodejs_test() {
  child_process.invoke_definition(
    "",
    "norm-check",
    host_definition("javascript", ",\"reuseKey\":\"not a safe key!\""),
    "{}",
    1000,
    1000,
  )
  |> expect_error_containing("reuseKey contains unsupported characters")
}

pub fn legacy_typescript_normalizes_to_nodejs_test() {
  child_process.invoke_definition(
    "",
    "norm-check",
    host_definition("typescript", ",\"reuseKey\":\"not a safe key!\""),
    "{}",
    1000,
    1000,
  )
  |> expect_error_containing("reuseKey contains unsupported characters")
}

pub fn missing_runtime_defaults_to_nodejs_test() {
  // No runtime field at all: the definition defaults to the nodejs pool and
  // therefore reaches the reuseKey validation rather than the runtime gate.
  child_process.invoke_definition(
    "",
    "norm-check",
    "{"
      <> "\"slug\":\"norm-check\","
      <> "\"containerized\":false,"
      <> "\"reuseKey\":\"not a safe key!\""
      <> "}",
    "{}",
    1000,
    1000,
  )
  |> expect_error_containing("reuseKey contains unsupported characters")
}

pub fn unknown_runtime_is_rejected_verbatim_test() {
  expect_error_containing(
    invoke_runtime("cobol"),
    "unsupported lambda runtime: cobol",
  )
}

// --- Identifier validation (before any database access) -------------------

pub fn invoke_rejects_invalid_identifier_before_db_lookup_test() {
  // Neither a UUID nor a lowercase slug: fails identifier validation before
  // LAMBDA_DATABASE_URL is even consulted.
  child_process.invoke("", "Not A Valid Identifier!!", "{}", 1000, 1000)
  |> expect_error_containing("valid lambda function UUID or slug is required")
}

// --- Pool-dispatch config validation (before any NATS publish) ------------

pub fn pool_language_token_is_validated_test() {
  child_process.invoke_definition(
    "",
    "norm-check",
    host_definition(
      "nodejs",
      ",\"poolBacked\":true,\"poolLanguage\":\"bad lang!\"",
    ),
    "{}",
    1000,
    1000,
  )
  |> expect_error_containing("invalid pool language token")
}

pub fn pool_subject_rejects_wire_unsafe_values_test() {
  // Whitespace would smuggle extra tokens into the NATS PUB line.
  child_process.invoke_definition(
    "",
    "norm-check",
    host_definition(
      "nodejs",
      ",\"poolBacked\":true,\"poolSubject\":\"dd.remote injected\"",
    ),
    "{}",
    1000,
    1000,
  )
  |> expect_error_containing("pool subject is not a valid NATS subject")
}

pub fn pool_slug_characters_are_validated_test() {
  child_process.invoke_definition(
    "",
    "norm-check",
    host_definition("nodejs", ",\"poolBacked\":true,\"poolSlug\":\"bad slug!\""),
    "{}",
    1000,
    1000,
  )
  |> expect_error_containing("poolSlug contains unsupported characters")
}

// --- Container image validation (before any runner is spawned) ------------

pub fn container_image_characters_are_validated_test() {
  child_process.invoke_definition(
    "",
    "norm-check",
    "{"
      <> "\"slug\":\"norm-check\","
      <> "\"runtime\":\"nodejs\","
      <> "\"containerized\":true,"
      <> "\"containerBuildStatus\":\"built\","
      <> "\"containerImage\":\"bad image; rm -rf /\""
      <> "}",
    "{}",
    1000,
    1000,
  )
  |> expect_error_containing("containerImage contains unsupported characters")
}

pub fn entry_command_is_passed_as_one_shell_quoted_environment_value_test() {
  let definition =
    "{"
    <> "\"runtime\":\"rust\","
    <> "\"containerized\":true,"
    <> "\"entryCommand\":\"printf '%s' \\\"$HOME\\\"; echo inside\","
    <> "\"containerBuildStatus\":\"built\","
    <> "\"containerImage\":\"ghcr.io/scintilla-run/custom@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\""
    <> "}"
  let assert Ok(command) =
    child_process.container_command_for_test("rust", definition)
  string.contains(
    command,
    "--env 'SCINTILLA_RUNTIME_COMMAND=printf '\"'\"'%s'\"'\"' \"$HOME\"; echo inside'",
  )
  |> should.be_true
  string.contains(
    command,
    "--tmpfs '/work:rw,exec,nosuid,nodev,size=1g,mode=1777'",
  )
  |> should.be_true
  string.contains(command, " ghcr.io/scintilla-run/custom")
  |> should.be_false
}

pub fn entry_command_length_is_bounded_before_a_container_starts_test() {
  let command = string.repeat("x", 513)
  let definition =
    "{"
    <> "\"runtime\":\"rust\","
    <> "\"containerized\":true,"
    <> "\"entryCommand\":\""
    <> command
    <> "\""
    <> "}"
  child_process.container_command_for_test("rust", definition)
  |> expect_error_containing("entryCommand contains unsupported characters")
}
