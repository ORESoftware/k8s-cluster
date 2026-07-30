import 'package:oresoftware_k8s_api_sdk_public/dd_api_sdk.dart';

void main() {
  if (sdkScope != "public") throw StateError('scope drift');
  if (catalogSha256 != "dd6cd2dfce13705381cfcc79c99d9f2b7e7307fc846e28609defe908e9859689") throw StateError('catalog drift');
  if (operations.length != 279) throw StateError('operation count drift');
  final ApiRequest request = buildRequest(
    baseUrl: 'https://example.test/',
    operationId: "agent_worker_broker_rs_get_api_docs_2fc0dbab70df",
  );
  if (request.method != "GET") throw StateError('method drift');
  if (request.url != "https://example.test/api/docs") throw StateError('URL drift');
}
