import 'package:oresoftware_k8s_api_sdk_public/dd_api_sdk.dart';

void main() {
  if (sdkScope != "public") throw StateError('scope drift');
  if (catalogSha256 != "6efa34dcaa32999c9265b34c503b99e9a824be3cda1c398871bb7a174f8e7721") throw StateError('catalog drift');
  if (operations.length != 281) throw StateError('operation count drift');
  final ApiRequest request = buildRequest(
    baseUrl: 'https://example.test/',
    operationId: "agent_worker_broker_rs_get_api_docs_2fc0dbab70df",
  );
  if (request.method != "GET") throw StateError('method drift');
  if (request.url != "https://example.test/api/docs") throw StateError('URL drift');
}
