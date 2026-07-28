import 'package:oresoftware_k8s_api_sdk_public/dd_api_sdk.dart';

void main() {
  if (sdkScope != "public") throw StateError('scope drift');
  if (catalogSha256 != "f1c583218baeeb1998f1ddf927b48d0868bfe3e7fbf04570d46309f149c824a1") throw StateError('catalog drift');
  if (operations.length != 274) throw StateError('operation count drift');
  final ApiRequest request = buildRequest(
    baseUrl: 'https://example.test/',
    operationId: "agent_worker_broker_rs_get_api_docs_2fc0dbab70df",
  );
  if (request.method != "GET") throw StateError('method drift');
  if (request.url != "https://example.test/api/docs") throw StateError('URL drift');
}
