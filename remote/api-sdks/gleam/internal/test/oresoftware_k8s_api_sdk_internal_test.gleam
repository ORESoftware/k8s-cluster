import dd_api_sdk
import gleam/list
import gleam/option.{None}
import gleeunit

pub fn main() {
  gleeunit.main()
}

pub fn builds_canonical_docs_request_test() {
  assert dd_api_sdk.sdk_scope == "internal"
  assert dd_api_sdk.catalog_sha256 == "5e3d4d825b548fbb43ca4272426f2a10808ecb0588fedfcbdc3f1ded723b51da" // gitleaks:allow
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
