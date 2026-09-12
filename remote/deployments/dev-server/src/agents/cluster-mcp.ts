export const CLUSTER_MCP_SERVER_NAME = 'dd_cluster';

export const CLUSTER_MCP_TOOL_NAMES = [
  'cluster_status',
  'service_directory',
  'kubernetes_inventory',
  'kubernetes_deployments',
  'human_access_policy',
  'telemetry_targets',
  'telemetry_summary',
  'observability_health',
  'prometheus_up',
  'loki_labels',
  'grafana_inventory',
  'nats_metrics',
  'trace_backends',
] as const;

export function clusterMcpUrlFromEnv(env: Record<string, string | undefined>): string | undefined {
  if (env.AGENT_MCP_ENABLED === 'false') {
    return undefined;
  }
  const url = env.AGENT_MCP_URL?.trim();
  return url || undefined;
}

export function clusterMcpConnectTimeoutMs(env: Record<string, string | undefined>): number {
  const parsed = Number(env.AGENT_MCP_CONNECT_TIMEOUT_MS ?? 3000);
  return Number.isFinite(parsed) && parsed > 0 ? Math.min(parsed, 30_000) : 3000;
}

export function clusterMcpPromptSection(url: string | null | undefined): string {
  if (!url) {
    return '';
  }

  return [
    `A read-only DD EC2 Kubernetes cluster MCP server is configured as ${CLUSTER_MCP_SERVER_NAME}.`,
    `Endpoint: ${url}`,
    'Use it before guessing Kubernetes runtime state, deployment inventory, service wiring, telemetry, or logs.',
    `Available tools: ${CLUSTER_MCP_TOOL_NAMES.join(', ')}.`,
    'The kubernetes_inventory tool reads bounded Kubernetes metadata across namespaces via the MCP service account.',
    'The MCP server is read-only by default; human access for sensitive operations goes through the authenticated gateway, VPN, and bastion flow.',
  ].join('\n');
}

export function clusterMcpInstructions(): string {
  return (
    `A read-only MCP server named ${CLUSTER_MCP_SERVER_NAME} may be available. ` +
    'Use its tools for cluster deployment inventory, service discovery, and observability before relying on guesses.'
  );
}
