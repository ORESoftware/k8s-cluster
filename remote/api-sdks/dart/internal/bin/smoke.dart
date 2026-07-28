import 'package:oresoftware_k8s_api_sdk_internal/dd_api_sdk.dart';

void main() {
  if (sdkScope != "internal") throw StateError('scope drift');
  if (catalogSha256 != "936fd83e3f4c565e871b07b4608d3191ab9a0235d5d4a970bf0d613854e18b94") throw StateError('catalog drift');
  if (operations.length != 914) throw StateError('operation count drift');
  final ApiRequest request = buildRequest(
    baseUrl: 'https://example.test/',
    operationId: "agent_worker_broker_rs_get_api_docs_2fc0dbab70df",
  );
  if (request.method != "GET") throw StateError('method drift');
  if (request.url != "https://example.test/api/docs") throw StateError('URL drift');
}
