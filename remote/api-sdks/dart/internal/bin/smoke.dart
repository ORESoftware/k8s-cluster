import 'package:oresoftware_k8s_api_sdk_internal/dd_api_sdk.dart';

void main() {
  if (sdkScope != "internal") throw StateError('scope drift');
  if (catalogSha256 != "d20ebafb4afe13d8d63c370d4e0ad98262b17faffb751a756b6ca3b946b9030c") throw StateError('catalog drift');
  if (operations.length != 937) throw StateError('operation count drift');
  final ApiRequest request = buildRequest(
    baseUrl: 'https://example.test/',
    operationId: "agent_worker_broker_rs_get_api_docs_2fc0dbab70df",
  );
  if (request.method != "GET") throw StateError('method drift');
  if (request.url != "https://example.test/api/docs") throw StateError('URL drift');
}
