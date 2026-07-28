import 'package:oresoftware_k8s_api_sdk_public/dd_api_sdk.dart';

void main() {
  if (sdkScope != "public") throw StateError('scope drift');
  if (catalogSha256 != "8b13a51a18433272657ee6c5e51b159c0a1d68a03ff6ae964d9bc553af5d2510") throw StateError('catalog drift');
  if (operations.length != 281) throw StateError('operation count drift');
  final ApiRequest request = buildRequest(
    baseUrl: 'https://example.test/',
    operationId: "agent_worker_broker_rs_get_api_docs_2fc0dbab70df",
  );
  if (request.method != "GET") throw StateError('method drift');
  if (request.url != "https://example.test/api/docs") throw StateError('URL drift');
}
