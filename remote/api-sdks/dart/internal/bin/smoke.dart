import 'package:oresoftware_k8s_api_sdk_internal/dd_api_sdk.dart';

void main() {
  if (sdkScope != "internal") throw StateError('scope drift');
  if (catalogSha256 != "44f420582b613723f6526d057fe1f7f87d999c0fa7558c1f4dc689b0cc6e143e") throw StateError('catalog drift');
  if (operations.length != 942) throw StateError('operation count drift');
  final ApiRequest request = buildRequest(
    baseUrl: 'https://example.test/',
    operationId: "agent_worker_broker_rs_get_api_docs_2fc0dbab70df",
  );
  if (request.method != "GET") throw StateError('method drift');
  if (request.url != "https://example.test/api/docs") throw StateError('URL drift');
}
