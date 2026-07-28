import 'package:oresoftware_k8s_api_sdk_internal/dd_api_sdk.dart';

void main() {
  if (sdkScope != "internal") throw StateError('scope drift');
  if (catalogSha256 != "94540173dd010e9faf26ab0996911bc31a0fdc75ba1a460295154cde51a04939") throw StateError('catalog drift');
  if (operations.length != 940) throw StateError('operation count drift');
  final ApiRequest request = buildRequest(
    baseUrl: 'https://example.test/',
    operationId: "agent_worker_broker_rs_get_api_docs_2fc0dbab70df",
  );
  if (request.method != "GET") throw StateError('method drift');
  if (request.url != "https://example.test/api/docs") throw StateError('URL drift');
}
