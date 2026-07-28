import dd_api_sdk
import gleam/list
import gleam/option.{None}
import gleeunit

pub fn main() {
  gleeunit.main()
}

pub fn builds_canonical_docs_request_test() {
  assert dd_api_sdk.sdk_scope == "public"
  assert dd_api_sdk.catalog_sha256 == "f1c583218baeeb1998f1ddf927b48d0868bfe3e7fbf04570d46309f149c824a1" // gitleaks:allow
  assert list.length(dd_api_sdk.operations()) == 274
  let assert Ok(request) = dd_api_sdk.build_request(
    "https://example.test/",
    "agent_worker_broker_rs_get_api_docs_2fc0dbab70df",
    [],
    [],
    [],
    None,
  )
  assert request.method == "GET"
  assert request.url == "https://example.test/api/docs"
}
