import dd_api_sdk
import gleam/list
import gleam/option.{None}
import gleeunit

pub fn main() {
  gleeunit.main()
}

pub fn builds_canonical_docs_request_test() {
  assert dd_api_sdk.sdk_scope == "internal"
  assert dd_api_sdk.catalog_sha256 == "44f420582b613723f6526d057fe1f7f87d999c0fa7558c1f4dc689b0cc6e143e" // gitleaks:allow
  assert list.length(dd_api_sdk.operations()) == 942
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
