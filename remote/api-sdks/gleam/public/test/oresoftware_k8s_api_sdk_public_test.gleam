import dd_api_sdk
import gleam/list
import gleam/option.{None}
import gleeunit

pub fn main() {
  gleeunit.main()
}

pub fn builds_canonical_docs_request_test() {
  assert dd_api_sdk.sdk_scope == "public"
  assert dd_api_sdk.catalog_sha256 == "8606866ed2947b927ad029b6209a16b2e6327f54bba830abe7d9cf33dea015ec" // gitleaks:allow
  assert list.length(dd_api_sdk.operations()) == 279
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
