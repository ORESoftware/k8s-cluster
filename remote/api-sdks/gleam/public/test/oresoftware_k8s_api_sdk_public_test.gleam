import dd_api_sdk
import gleam/list
import gleam/option.{None}
import gleeunit

pub fn main() {
  gleeunit.main()
}

pub fn builds_canonical_docs_request_test() {
  assert dd_api_sdk.sdk_scope == "public"
  assert dd_api_sdk.catalog_sha256 == "a4b15b4d00a5a70d8b985325ba2008f1aade9c7ec165cc95015e29260c9cfaf2" // gitleaks:allow
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
