//! The MCP server: tool routing and the `ServerHandler` implementation.

use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router, ErrorData, ServerHandler,
};
use serde::Deserialize;

use crate::tools::{cloudflare, docs, domain, github, health, k8s};

pub struct CanonicalMcp {
    http: reqwest::Client,
    github: github::GitHubClient,
    tool_router: ToolRouter<Self>,
}

impl CanonicalMcp {
    pub fn new() -> Result<Self, reqwest::Error> {
        let http = reqwest::Client::builder()
            .user_agent(github::USER_AGENT)
            .build()?;
        Ok(Self {
            github: github::GitHubClient::new(http.clone()),
            http,
            tool_router: Self::tool_router(),
        })
    }
}

fn json_result(value: &impl serde::Serialize) -> Result<CallToolResult, ErrorData> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

fn tool_error(message: String) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message)])
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct StackCiStatusParams {
    /// Limit the report to one repository (e.g. "canonical-web-server.rs").
    /// Omit to report on every stack repository.
    pub repo: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ServiceHealthParams {
    /// Base URL of the deployment to probe, e.g. "https://canonical.cloud".
    pub base_url: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct StackDocsParams {
    /// Which document to fetch: "deploy" or "repo-boundaries".
    pub doc: docs::DocName,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DomainStatusParams {
    /// Domain to inspect. Defaults to "canonical.cloud".
    pub domain: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CloudflareDnsParams {
    /// Zone (apex domain) whose DNS records to list. Defaults to "canonical.cloud".
    pub domain: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct K8sStatusParams {
    /// Resource to inspect: nodes, pods, deployments, services, or ingresses.
    pub resource: k8s::K8sResource,
    /// Namespace to scope to. Omit for all namespaces.
    pub namespace: Option<String>,
    /// kubeconfig context to use. Omit for the current context.
    pub context: Option<String>,
}

#[tool_router]
impl CanonicalMcp {
    #[tool(
        description = "Latest GitHub Actions runs for each canonical-cloud stack repository \
                       (canonical-monorepo, canonical-web-server.rs, canonical-marketing-site.web, \
                       canonical-interfaces). Returns repo, workflow, branch, status, conclusion, \
                       run URL, and timestamp for the five most recent runs per repo."
    )]
    async fn stack_ci_status(
        &self,
        Parameters(params): Parameters<StackCiStatusParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match github::ci_status_report(&self.github, params.repo.as_deref()).await {
            Ok(report) => json_result(&report),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Compare canonical-monorepo's submodule pins under apps/ against each app \
                       repository's main HEAD. Reports the pinned SHA, main HEAD SHA, whether the \
                       pin is current, and how many commits behind it is."
    )]
    async fn submodule_pins(&self) -> Result<CallToolResult, ErrorData> {
        match github::submodule_pins_report(&self.github).await {
            Ok(report) => json_result(&report),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Probe a canonical.cloud deployment's health endpoints (/healthz, /readyz, \
                       /api/v1/health) and return status codes and truncated bodies."
    )]
    async fn service_health(
        &self,
        Parameters(params): Parameters<ServiceHealthParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match health::probe(&self.http, &params.base_url).await {
            Ok(reports) => json_result(&reports),
            Err(error) => Err(ErrorData::invalid_params(error, None)),
        }
    }

    #[tool(
        description = "Fetch canonical-monorepo operational docs as raw markdown. \
                       doc = \"deploy\" (docs/deploy.md) or \"repo-boundaries\" \
                       (docs/repo-boundaries.md)."
    )]
    async fn stack_docs(
        &self,
        Parameters(params): Parameters<StackDocsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match docs::fetch(&self.http, params.doc).await {
            Ok(markdown) => Ok(CallToolResult::success(vec![ContentBlock::text(markdown)])),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Registrar-side and DNS-side status for a domain (default canonical.cloud). \
                       Registrar state (Squarespace has no public domains API) comes from public \
                       RDAP: registrar, status codes, registration/expiration events, delegated \
                       nameservers. Live NS/A/AAAA resolution comes from DNS-over-HTTPS, plus \
                       whether the delegation points at Cloudflare."
    )]
    async fn domain_status(
        &self,
        Parameters(params): Parameters<DomainStatusParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let domain = params.domain.as_deref().unwrap_or(domain::DEFAULT_DOMAIN);
        match domain::domain_status_report(&self.http, domain).await {
            Ok(report) => json_result(&report),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "List DNS records for a Cloudflare zone (default canonical.cloud): type, \
                       name, content, proxied, TTL. Read-only; requires a CLOUDFLARE_API_TOKEN \
                       env var with Zone.Zone:Read and Zone.DNS:Read."
    )]
    async fn cloudflare_dns(
        &self,
        Parameters(params): Parameters<CloudflareDnsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let domain = params.domain.as_deref().unwrap_or(domain::DEFAULT_DOMAIN);
        match cloudflare::dns_records_report(&self.http, domain).await {
            Ok(report) => json_result(&report),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Read-only Kubernetes status via `kubectl get` (nodes, pods, deployments, \
                       services, or ingresses), summarized to name/namespace/status/age rows. \
                       Optional namespace and kubeconfig context. Never mutates the cluster."
    )]
    async fn k8s_status(
        &self,
        Parameters(params): Parameters<K8sStatusParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match k8s::k8s_status_report(
            params.resource,
            params.namespace.as_deref(),
            params.context.as_deref(),
        )
        .await
        {
            Ok(report) => json_result(&report),
            Err(error) => Ok(tool_error(error)),
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for CanonicalMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "canonical-mcp",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "Operational tooling for the canonical.cloud stack (GitHub org canonical-cloud). \
                 Use stack_ci_status for CI health, submodule_pins to check monorepo pins, \
                 service_health to probe a deployment, stack_docs for deploy/repo-boundary \
                 documentation, domain_status for registrar (RDAP) and DNS delegation state, \
                 cloudflare_dns to list zone records (needs CLOUDFLARE_API_TOKEN), and \
                 k8s_status for read-only cluster state via kubectl. Set GITHUB_TOKEN (or \
                 GH_TOKEN) for higher GitHub rate limits.",
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn router_exposes_all_v1_tools() {
        let router = CanonicalMcp::tool_router();
        let names: Vec<String> = router
            .list_all()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect();
        assert_eq!(
            names,
            vec![
                "cloudflare_dns",
                "domain_status",
                "k8s_status",
                "service_health",
                "stack_ci_status",
                "stack_docs",
                "submodule_pins",
            ]
        );
    }

    #[test]
    fn stack_docs_schema_restricts_doc_enum() {
        let router = CanonicalMcp::tool_router();
        let tool = router
            .list_all()
            .into_iter()
            .find(|tool| tool.name == "stack_docs")
            .expect("stack_docs registered");
        let schema = serde_json::to_value(&tool.input_schema).expect("schema serializes");
        let text = schema.to_string();
        assert!(text.contains("deploy"), "schema mentions deploy: {text}");
        assert!(
            text.contains("repo-boundaries"),
            "schema mentions repo-boundaries: {text}"
        );
    }
}
