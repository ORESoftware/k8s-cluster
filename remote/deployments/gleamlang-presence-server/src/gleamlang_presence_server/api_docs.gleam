//// Native API documentation responses for the presence service.
////
//// The JSON is generated directly from `route_contract.routes()`, the same
//// registry used by the Mist dispatcher. The HTML reference is a thin Scalar
//// shell over that JSON and contains no second route/model inventory.

import gleam/bytes_tree
import gleam/http/response
import gleamlang_presence_server/route_contract
import mist

const scalar_html = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta name=\"color-scheme\" content=\"light dark\"><title>gleamlang-presence-server API</title></head><body><div id=\"app\"></div><script src=\"https://cdn.jsdelivr.net/npm/@scalar/api-reference@1.62.4\"></script><script>Scalar.createApiReference('#app',{url:'/openapi.json',theme:'default',layout:'modern',hideModels:false,hideDownloadButton:false,showSidebar:true});</script></body></html>"

pub fn html() -> response.Response(mist.ResponseData) {
  response.new(200)
  |> response.set_header("content-type", "text/html; charset=utf-8")
  |> response.set_header("cache-control", "no-store")
  |> response.set_header("x-content-type-options", "nosniff")
  |> response.set_body(mist.Bytes(bytes_tree.from_string(scalar_html)))
}

pub fn json() -> response.Response(mist.ResponseData) {
  response.new(200)
  |> response.set_header("content-type", "application/json; charset=utf-8")
  |> response.set_header("cache-control", "no-store")
  |> response.set_header("x-content-type-options", "nosniff")
  |> response.set_body(
    route_contract.public_openapi_json()
    |> bytes_tree.from_string
    |> mist.Bytes,
  )
}
