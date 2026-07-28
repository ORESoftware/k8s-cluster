import 'package:oresoftware_k8s_api_sdk_internal/dd_api_sdk.dart';

void main() {
  if (sdkScope != "internal") throw StateError('scope drift');
  if (catalogSha256 != "5ea0759ea8117e34fbf44478e27072f5fd02b40675bfca80e586f166015d16f4") throw StateError('catalog drift');
  if (operations.length != 940) throw StateError('operation count drift');
  final ApiRequest request = buildRequest(
    baseUrl: 'https://example.test/',
    operationId: "agent_worker_broker_rs_get_api_docs_2fc0dbab70df",
  );
  if (request.method != "GET") throw StateError('method drift');
  if (request.url != "https://example.test/api/docs") throw StateError('URL drift');
}
