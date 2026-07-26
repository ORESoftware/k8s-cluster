//! OAuth 2.1 authorization server and MCP protected-resource integration.
//!
//! The service is both the MCP resource server and its authorization server.
//! Public clients register dynamically, use authorization-code + PKCE S256,
//! and receive audience-bound, scoped access tokens. Authorization codes and
//! rotating refresh grants are opaque and single-use in Redis so both replicas
//! share replay protection. Access tokens are short-lived HMAC-signed JWTs and
//! can still be validated during a brief Redis outage.

use std::{
    collections::BTreeSet,
    env,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{Form, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use url::Url;

use crate::{json_response, AppState, SERVICE_NAME};

type HmacSha256 = Hmac<Sha256>;

pub const SCOPE_MCP_TOOLS: &str = "mcp:tools";
pub const SCOPE_BROWSER_READ: &str = "browser:read";
pub const SCOPE_BROWSER_ACT: &str = "browser:act";
const SCOPE_OFFLINE_ACCESS: &str = "offline_access";
pub const RESOURCE_SCOPES: &[&str] = &[SCOPE_MCP_TOOLS, SCOPE_BROWSER_READ, SCOPE_BROWSER_ACT];
pub const INITIAL_RESOURCE_SCOPES: &[&str] = RESOURCE_SCOPES;
const ALL_SCOPES: &[&str] = &[
    SCOPE_MCP_TOOLS,
    SCOPE_BROWSER_READ,
    SCOPE_BROWSER_ACT,
    SCOPE_OFFLINE_ACCESS,
];
const REDIS_PREFIX: &str = "dd:browser-mcp:oauth:v1";
const MAX_REDIRECT_URIS: usize = 10;
const MAX_REDIRECT_URI_CHARS: usize = 2048;
const MAX_CLIENT_NAME_CHARS: usize = 200;

#[derive(Clone)]
pub struct OAuthService {
    public_base_urls: Vec<Url>,
    signing_secret: Vec<u8>,
    operator_secret: String,
    redis: redis::Client,
    access_ttl_secs: u64,
    code_ttl_secs: u64,
    refresh_ttl_secs: u64,
}

#[derive(Debug)]
pub struct Principal {
    pub owner: String,
}

#[derive(Debug)]
pub enum AccessError {
    MissingToken,
    InvalidToken,
    InsufficientScope,
    InvalidExternalOrigin,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RegisteredClient {
    issuer: String,
    redirect_uris: Vec<String>,
    client_name: String,
    issued_at: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct AuthorizationRequestEnvelope {
    issuer: String,
    client_id: String,
    redirect_uri: String,
    scope: String,
    resource: String,
    code_challenge: String,
    state: Option<String>,
    expires_at: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct AuthorizationCodeGrant {
    issuer: String,
    subject: String,
    client_id: String,
    redirect_uri: String,
    scope: String,
    resource: String,
    code_challenge: String,
    expires_at: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct RefreshGrant {
    issuer: String,
    subject: String,
    client_id: String,
    scope: String,
    resource: String,
    expires_at: u64,
    family_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct AccessTokenClaims {
    iss: String,
    sub: String,
    aud: String,
    client_id: String,
    scope: String,
    iat: u64,
    nbf: u64,
    exp: u64,
    jti: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct JwtHeader {
    alg: String,
    typ: String,
}

#[derive(Debug, Deserialize)]
pub struct ClientRegistrationRequest {
    redirect_uris: Vec<String>,
    #[serde(default)]
    client_name: Option<String>,
    #[serde(default)]
    grant_types: Vec<String>,
    #[serde(default)]
    response_types: Vec<String>,
    #[serde(default)]
    token_endpoint_auth_method: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AuthorizationRequest {
    response_type: String,
    client_id: String,
    redirect_uri: String,
    code_challenge: String,
    code_challenge_method: String,
    resource: String,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    state: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AuthorizationDecision {
    request: String,
    operator_secret: String,
    #[serde(default)]
    approve: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TokenRequest {
    grant_type: String,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    redirect_uri: Option<String>,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    code_verifier: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    resource: Option<String>,
}

impl OAuthService {
    pub fn from_env() -> Result<Self, String> {
        let public_base_urls = env::var("BROWSER_MCP_PUBLIC_BASE_URLS")
            .map_err(|_| "BROWSER_MCP_PUBLIC_BASE_URLS is required in OAuth mode".to_string())?
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(parse_public_base_url)
            .collect::<Result<Vec<_>, _>>()?;
        if public_base_urls.is_empty() {
            return Err("BROWSER_MCP_PUBLIC_BASE_URLS must contain at least one URL".to_string());
        }

        let signing_secret = env::var("BROWSER_MCP_OAUTH_SIGNING_SECRET")
            .map_err(|_| "BROWSER_MCP_OAUTH_SIGNING_SECRET is required in OAuth mode".to_string())?
            .into_bytes();
        if signing_secret.len() < 32 {
            return Err("BROWSER_MCP_OAUTH_SIGNING_SECRET must be at least 32 bytes".to_string());
        }
        let operator_secret = env::var("BROWSER_MCP_OAUTH_OPERATOR_SECRET").map_err(|_| {
            "BROWSER_MCP_OAUTH_OPERATOR_SECRET is required in OAuth mode".to_string()
        })?;
        if operator_secret.len() < 20 {
            return Err("BROWSER_MCP_OAUTH_OPERATOR_SECRET must be at least 20 bytes".to_string());
        }
        let redis_url = env::var("BROWSER_MCP_OAUTH_REDIS_URL").unwrap_or_else(|_| {
            "redis://dd-redis-cache.default.svc.cluster.local:6379/4".to_string()
        });
        let redis = redis::Client::open(redis_url)
            .map_err(|error| format!("invalid BROWSER_MCP_OAUTH_REDIS_URL: {error}"))?;

        Ok(Self {
            public_base_urls,
            signing_secret,
            operator_secret,
            redis,
            access_ttl_secs: env_u64("BROWSER_MCP_OAUTH_ACCESS_TTL_SECONDS", 900, 3600),
            code_ttl_secs: env_u64("BROWSER_MCP_OAUTH_CODE_TTL_SECONDS", 300, 600),
            refresh_ttl_secs: env_u64(
                "BROWSER_MCP_OAUTH_REFRESH_TTL_SECONDS",
                2_592_000,
                7_776_000,
            ),
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test(base: &str, signing_secret: &str, operator_secret: &str) -> Self {
        Self {
            public_base_urls: vec![parse_public_base_url(base).unwrap()],
            signing_secret: signing_secret.as_bytes().to_vec(),
            operator_secret: operator_secret.to_string(),
            redis: redis::Client::open("redis://127.0.0.1:6379/15").unwrap(),
            access_ttl_secs: 900,
            code_ttl_secs: 300,
            refresh_ttl_secs: 2_592_000,
        }
    }

    pub async fn store_ready(&self) -> bool {
        let Ok(mut connection) = self.redis.get_multiplexed_async_connection().await else {
            return false;
        };
        redis::cmd("PING")
            .query_async::<String>(&mut connection)
            .await
            .is_ok_and(|reply| reply == "PONG")
    }

    fn external_base<'a>(&'a self, headers: &HeaderMap) -> Result<&'a Url, AccessError> {
        let host = headers
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if let Some(host) = host {
            if let Some(base) = self
                .public_base_urls
                .iter()
                .find(|base| authority(base).eq_ignore_ascii_case(host))
            {
                return Ok(base);
            }
        }
        if self.public_base_urls.len() == 1 {
            return Ok(&self.public_base_urls[0]);
        }
        Err(AccessError::InvalidExternalOrigin)
    }

    fn resource_metadata_url(base: &Url) -> String {
        let mut metadata = base.clone();
        let resource_path = base.path().trim_matches('/');
        metadata.set_path(&format!(
            "/.well-known/oauth-protected-resource/{resource_path}"
        ));
        metadata.to_string()
    }

    fn authorization_endpoint(base: &Url) -> String {
        format!("{}/oauth/authorize", base.as_str().trim_end_matches('/'))
    }

    fn token_endpoint(base: &Url) -> String {
        format!("{}/oauth/token", base.as_str().trim_end_matches('/'))
    }

    fn registration_endpoint(base: &Url) -> String {
        format!("{}/oauth/register", base.as_str().trim_end_matches('/'))
    }

    pub fn protected_resource_metadata(&self, headers: &HeaderMap) -> Result<Value, AccessError> {
        let base = self.external_base(headers)?;
        Ok(json!({
            "resource": base.as_str(),
            "authorization_servers": [base.as_str()],
            "bearer_methods_supported": ["header"],
            "scopes_supported": RESOURCE_SCOPES,
            "resource_name": "DD Browser Automation MCP Server"
        }))
    }

    pub fn authorization_server_metadata(&self, headers: &HeaderMap) -> Result<Value, AccessError> {
        let base = self.external_base(headers)?;
        Ok(json!({
            "issuer": base.as_str(),
            "authorization_endpoint": Self::authorization_endpoint(base),
            "token_endpoint": Self::token_endpoint(base),
            "registration_endpoint": Self::registration_endpoint(base),
            "scopes_supported": ALL_SCOPES,
            "response_types_supported": ["code"],
            "response_modes_supported": ["query"],
            "grant_types_supported": ["authorization_code", "refresh_token"],
            "token_endpoint_auth_methods_supported": ["none"],
            "code_challenge_methods_supported": ["S256"],
            "service_documentation": format!("{}/", base.as_str().trim_end_matches('/'))
        }))
    }

    fn register_client(
        &self,
        headers: &HeaderMap,
        request: ClientRegistrationRequest,
    ) -> Result<Value, OAuthProtocolError> {
        let base = self
            .external_base(headers)
            .map_err(|_| OAuthProtocolError::invalid_request("unrecognized public origin"))?;
        validate_registration_request(&request)?;
        let client_name = request
            .client_name
            .unwrap_or_else(|| "ChatGPT MCP client".to_string());
        let client = RegisteredClient {
            issuer: base.as_str().to_string(),
            redirect_uris: request.redirect_uris,
            client_name: client_name.chars().take(MAX_CLIENT_NAME_CHARS).collect(),
            issued_at: now(),
        };
        let client_id = self.seal("client", &client)?;
        Ok(json!({
            "client_id": client_id,
            "client_id_issued_at": client.issued_at,
            "redirect_uris": client.redirect_uris,
            "client_name": client.client_name,
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none"
        }))
    }

    fn validate_authorization_request(
        &self,
        headers: &HeaderMap,
        request: AuthorizationRequest,
    ) -> Result<(RegisteredClient, AuthorizationRequestEnvelope), OAuthProtocolError> {
        let base = self
            .external_base(headers)
            .map_err(|_| OAuthProtocolError::invalid_request("unrecognized public origin"))?;
        if request.response_type != "code" {
            return Err(OAuthProtocolError::new(
                "unsupported_response_type",
                "only response_type=code is supported",
            ));
        }
        if request.code_challenge_method != "S256" || !valid_pkce_value(&request.code_challenge) {
            return Err(OAuthProtocolError::invalid_request(
                "PKCE code_challenge_method=S256 is required",
            ));
        }
        if request
            .state
            .as_deref()
            .is_none_or(|state| !(16..=512).contains(&state.len()))
        {
            return Err(OAuthProtocolError::invalid_request(
                "state is required and must contain between 16 and 512 bytes",
            ));
        }
        if request.resource != base.as_str() {
            return Err(OAuthProtocolError::invalid_target(
                "resource must equal the canonical MCP endpoint",
            ));
        }
        let client: RegisteredClient = self.unseal("client", &request.client_id)?;
        if client.issuer != base.as_str()
            || !client
                .redirect_uris
                .iter()
                .any(|registered| registered == &request.redirect_uri)
        {
            return Err(OAuthProtocolError::invalid_request(
                "client_id or redirect_uri is not registered for this issuer",
            ));
        }
        let scope = normalize_scope(request.scope.as_deref())?;
        Ok((
            client,
            AuthorizationRequestEnvelope {
                issuer: base.as_str().to_string(),
                client_id: request.client_id,
                redirect_uri: request.redirect_uri,
                scope,
                resource: request.resource,
                code_challenge: request.code_challenge,
                state: request.state,
                expires_at: now() + self.code_ttl_secs,
            },
        ))
    }

    fn authorization_page(
        &self,
        headers: &HeaderMap,
        request: AuthorizationRequest,
    ) -> Result<Response, OAuthProtocolError> {
        let (client, envelope) = self.validate_authorization_request(headers, request)?;
        let sealed_request = self.seal("authorization-request", &envelope)?;
        let scope_items = envelope
            .scope
            .split_whitespace()
            .map(|scope| format!("<li><code>{}</code></li>", html_escape(scope)))
            .collect::<String>();
        let page = format!(
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
             <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
             <title>Authorize DD Browser MCP</title>\
             <style>body{{font:16px system-ui,sans-serif;max-width:42rem;margin:4rem auto;padding:0 1.25rem;line-height:1.5}}\
             code{{background:#eee;padding:.1rem .3rem;border-radius:.2rem}}input[type=password]{{width:100%;padding:.7rem;box-sizing:border-box}}\
             button{{padding:.7rem 1rem;margin-top:1rem}}.meta{{color:#555;overflow-wrap:anywhere}}</style></head>\
             <body><h1>Authorize browser automation</h1>\
             <p><strong>{}</strong> is requesting access to the DD browser MCP server.</p>\
             <p class=\"meta\">Redirect: {}</p><p>Requested permissions:</p><ul>{}</ul>\
             <p>This can navigate and fill forms on the server's configured domain allowlist. \
             Consequential submissions still require a separate confirmation.</p>\
             <form method=\"post\" action=\"{}\">\
             <input type=\"hidden\" name=\"request\" value=\"{}\">\
             <label>Operator authorization secret\
             <input name=\"operator_secret\" type=\"password\" autocomplete=\"current-password\" required></label>\
             <label><input name=\"approve\" type=\"checkbox\" value=\"yes\" required> I approve these permissions</label><br>\
             <button type=\"submit\">Authorize</button></form></body></html>",
            html_escape(&client.client_name),
            html_escape(&envelope.redirect_uri),
            scope_items,
            html_escape(&Self::authorization_endpoint(
                &Url::parse(&envelope.issuer).expect("validated issuer")
            )),
            html_escape(&sealed_request),
        );
        Ok(html_response(StatusCode::OK, page))
    }

    async fn authorization_decision(
        &self,
        headers: &HeaderMap,
        decision: AuthorizationDecision,
    ) -> Result<Response, OAuthProtocolError> {
        let envelope: AuthorizationRequestEnvelope =
            self.unseal("authorization-request", &decision.request)?;
        let base = self
            .external_base(headers)
            .map_err(|_| OAuthProtocolError::invalid_request("unrecognized public origin"))?;
        if envelope.issuer != base.as_str() || envelope.expires_at < now() {
            return Err(OAuthProtocolError::invalid_request(
                "authorization request expired or was issued for another origin",
            ));
        }
        let expected_secret_hash = Sha256::digest(self.operator_secret.as_bytes());
        let provided_secret_hash = Sha256::digest(decision.operator_secret.as_bytes());
        let secret_ok: bool = expected_secret_hash.ct_eq(&provided_secret_hash).into();
        if !secret_ok || decision.approve.as_deref() != Some("yes") {
            return redirect_with_oauth_result(
                &envelope.redirect_uri,
                &[("error", "access_denied")],
                envelope.state.as_deref(),
            );
        }

        let code = random_token(32);
        let subject = format!("operator:{}", short_hash(&envelope.client_id));
        let grant = AuthorizationCodeGrant {
            issuer: envelope.issuer,
            subject,
            client_id: envelope.client_id,
            redirect_uri: envelope.redirect_uri.clone(),
            scope: envelope.scope,
            resource: envelope.resource,
            code_challenge: envelope.code_challenge,
            expires_at: envelope.expires_at,
        };
        self.store_once("code", &code, &grant, self.code_ttl_secs)
            .await?;
        redirect_with_oauth_result(
            &envelope.redirect_uri,
            &[("code", code.as_str())],
            envelope.state.as_deref(),
        )
    }

    pub async fn token(&self, headers: &HeaderMap, request: TokenRequest) -> Response {
        let result = match request.grant_type.as_str() {
            "authorization_code" => self.exchange_authorization_code(headers, request).await,
            "refresh_token" => self.exchange_refresh_token(headers, request).await,
            _ => Err(OAuthProtocolError::new(
                "unsupported_grant_type",
                "supported grant types are authorization_code and refresh_token",
            )),
        };
        match result {
            Ok(value) => no_store_json(StatusCode::OK, value),
            Err(error) => error.into_json_response(),
        }
    }

    async fn exchange_authorization_code(
        &self,
        headers: &HeaderMap,
        request: TokenRequest,
    ) -> Result<Value, OAuthProtocolError> {
        let base = self
            .external_base(headers)
            .map_err(|_| OAuthProtocolError::invalid_request("unrecognized public origin"))?;
        let code = required(request.code, "code")?;
        let client_id = required(request.client_id, "client_id")?;
        self.validate_client_for_issuer(&client_id, base.as_str())?;
        let redirect_uri = required(request.redirect_uri, "redirect_uri")?;
        let verifier = required(request.code_verifier, "code_verifier")?;
        let resource = required(request.resource, "resource")?;
        if !valid_pkce_value(&verifier) {
            return Err(OAuthProtocolError::invalid_grant("invalid PKCE verifier"));
        }

        let grant: AuthorizationCodeGrant = self.take_once("code", &code).await?;
        if grant.expires_at < now()
            || grant.issuer != base.as_str()
            || grant.client_id != client_id
            || grant.redirect_uri != redirect_uri
            || grant.resource != resource
            || !pkce_matches(&verifier, &grant.code_challenge)
        {
            return Err(OAuthProtocolError::invalid_grant(
                "authorization code validation failed",
            ));
        }
        self.issue_token_response(
            &grant.issuer,
            &grant.subject,
            &grant.client_id,
            &grant.scope,
            &grant.resource,
            None,
        )
        .await
    }

    async fn exchange_refresh_token(
        &self,
        headers: &HeaderMap,
        request: TokenRequest,
    ) -> Result<Value, OAuthProtocolError> {
        let base = self
            .external_base(headers)
            .map_err(|_| OAuthProtocolError::invalid_request("unrecognized public origin"))?;
        let refresh_token = required(request.refresh_token, "refresh_token")?;
        let client_id = required(request.client_id, "client_id")?;
        self.validate_client_for_issuer(&client_id, base.as_str())?;
        let grant: RefreshGrant = self.take_once("refresh", &refresh_token).await?;
        if grant.expires_at < now()
            || grant.issuer != base.as_str()
            || grant.client_id != client_id
            || request
                .resource
                .as_deref()
                .is_some_and(|resource| resource != grant.resource)
        {
            return Err(OAuthProtocolError::invalid_grant(
                "refresh token validation failed",
            ));
        }
        self.issue_token_response(
            &grant.issuer,
            &grant.subject,
            &grant.client_id,
            &grant.scope,
            &grant.resource,
            Some(grant.family_id),
        )
        .await
    }

    async fn issue_token_response(
        &self,
        issuer: &str,
        subject: &str,
        client_id: &str,
        scope: &str,
        resource: &str,
        refresh_family: Option<String>,
    ) -> Result<Value, OAuthProtocolError> {
        let issued_at = now();
        let claims = AccessTokenClaims {
            iss: issuer.to_string(),
            sub: subject.to_string(),
            aud: resource.to_string(),
            client_id: client_id.to_string(),
            scope: scope.to_string(),
            iat: issued_at,
            nbf: issued_at.saturating_sub(5),
            exp: issued_at + self.access_ttl_secs,
            jti: random_token(16),
        };
        let access_token = self.sign_access_token(&claims)?;
        let mut response = json!({
            "access_token": access_token,
            "token_type": "Bearer",
            "expires_in": self.access_ttl_secs,
            "scope": scope
        });
        if scope
            .split_whitespace()
            .any(|value| value == SCOPE_OFFLINE_ACCESS)
        {
            let raw_refresh = random_token(32);
            let family_id = refresh_family.unwrap_or_else(|| random_token(16));
            let refresh_grant = RefreshGrant {
                issuer: issuer.to_string(),
                subject: subject.to_string(),
                client_id: client_id.to_string(),
                scope: scope.to_string(),
                resource: resource.to_string(),
                expires_at: issued_at + self.refresh_ttl_secs,
                family_id,
            };
            self.store_once(
                "refresh",
                &raw_refresh,
                &refresh_grant,
                self.refresh_ttl_secs,
            )
            .await?;
            response["refresh_token"] = Value::String(raw_refresh);
        }
        Ok(response)
    }

    pub fn authenticate(
        &self,
        headers: &HeaderMap,
        required_scopes: &[&str],
    ) -> Result<Principal, AccessError> {
        let base = self.external_base(headers)?;
        let token = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .filter(|value| !value.is_empty())
            .ok_or(AccessError::MissingToken)?;
        let claims = self
            .verify_access_token(token)
            .map_err(|_| AccessError::InvalidToken)?;
        let current_time = now();
        if claims.iss != base.as_str()
            || claims.aud != base.as_str()
            || claims.exp <= current_time
            || claims.nbf > current_time + 5
        {
            return Err(AccessError::InvalidToken);
        }
        let scopes = claims
            .scope
            .split_whitespace()
            .map(str::to_string)
            .collect::<BTreeSet<_>>();
        if required_scopes
            .iter()
            .any(|required| !scopes.contains(*required))
        {
            return Err(AccessError::InsufficientScope);
        }
        Ok(Principal {
            owner: format!("oauth:{}", short_hash(&claims.sub)),
        })
    }

    pub fn challenge_response(
        &self,
        headers: &HeaderMap,
        error: AccessError,
        required_scopes: &[&str],
        body: Value,
    ) -> Response {
        let base = self.external_base(headers).ok();
        let metadata = base
            .map(Self::resource_metadata_url)
            .unwrap_or_else(|| "/.well-known/oauth-protected-resource/browser-mcp".to_string());
        let scope = required_scopes.join(" ");
        let (status, challenge) = match error {
            AccessError::InsufficientScope => (
                StatusCode::FORBIDDEN,
                format!(
                    "Bearer error=\"insufficient_scope\", scope=\"{}\", resource_metadata=\"{}\"",
                    scope, metadata
                ),
            ),
            AccessError::InvalidToken => (
                StatusCode::UNAUTHORIZED,
                format!(
                    "Bearer realm=\"{}\", error=\"invalid_token\", scope=\"{}\", resource_metadata=\"{}\"",
                    SERVICE_NAME, scope, metadata
                ),
            ),
            AccessError::MissingToken | AccessError::InvalidExternalOrigin => (
                StatusCode::UNAUTHORIZED,
                format!(
                    "Bearer realm=\"{}\", scope=\"{}\", resource_metadata=\"{}\"",
                    SERVICE_NAME, scope, metadata
                ),
            ),
        };
        let mut response = json_response(status, body);
        if let Ok(value) = HeaderValue::from_str(&challenge) {
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, value);
        }
        response
    }

    fn seal<T: Serialize>(&self, purpose: &str, value: &T) -> Result<String, OAuthProtocolError> {
        let payload = serde_json::to_vec(value).map_err(OAuthProtocolError::server_error)?;
        let encoded = URL_SAFE_NO_PAD.encode(payload);
        let signature = self.sign_bytes(format!("{purpose}.{encoded}").as_bytes());
        Ok(format!(
            "{purpose}.{encoded}.{}",
            URL_SAFE_NO_PAD.encode(signature)
        ))
    }

    fn validate_client_for_issuer(
        &self,
        client_id: &str,
        issuer: &str,
    ) -> Result<RegisteredClient, OAuthProtocolError> {
        let client: RegisteredClient = self
            .unseal("client", client_id)
            .map_err(|_| OAuthProtocolError::invalid_client("invalid public client_id"))?;
        if client.issuer != issuer {
            return Err(OAuthProtocolError::invalid_client(
                "client_id was registered for another issuer",
            ));
        }
        Ok(client)
    }

    fn unseal<T: DeserializeOwned>(
        &self,
        purpose: &str,
        value: &str,
    ) -> Result<T, OAuthProtocolError> {
        let mut parts = value.split('.');
        let Some(actual_purpose) = parts.next() else {
            return Err(OAuthProtocolError::invalid_request("invalid signed value"));
        };
        let Some(encoded) = parts.next() else {
            return Err(OAuthProtocolError::invalid_request("invalid signed value"));
        };
        let Some(signature) = parts.next() else {
            return Err(OAuthProtocolError::invalid_request("invalid signed value"));
        };
        if parts.next().is_some() || actual_purpose != purpose {
            return Err(OAuthProtocolError::invalid_request("invalid signed value"));
        }
        let signature = URL_SAFE_NO_PAD
            .decode(signature)
            .map_err(|_| OAuthProtocolError::invalid_request("invalid signed value"))?;
        if !self.verify_bytes(format!("{purpose}.{encoded}").as_bytes(), &signature) {
            return Err(OAuthProtocolError::invalid_request("invalid signed value"));
        }
        let payload = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| OAuthProtocolError::invalid_request("invalid signed value"))?;
        serde_json::from_slice(&payload)
            .map_err(|_| OAuthProtocolError::invalid_request("invalid signed value"))
    }

    fn sign_access_token(&self, claims: &AccessTokenClaims) -> Result<String, OAuthProtocolError> {
        let header = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&JwtHeader {
                alg: "HS256".to_string(),
                typ: "at+jwt".to_string(),
            })
            .map_err(OAuthProtocolError::server_error)?,
        );
        let claims = URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(claims).map_err(OAuthProtocolError::server_error)?);
        let signing_input = format!("{header}.{claims}");
        let signature = URL_SAFE_NO_PAD.encode(self.sign_bytes(signing_input.as_bytes()));
        Ok(format!("{signing_input}.{signature}"))
    }

    fn verify_access_token(&self, token: &str) -> Result<AccessTokenClaims, ()> {
        let mut parts = token.split('.');
        let (Some(header), Some(claims), Some(signature)) =
            (parts.next(), parts.next(), parts.next())
        else {
            return Err(());
        };
        if parts.next().is_some() {
            return Err(());
        }
        let signature = URL_SAFE_NO_PAD.decode(signature).map_err(|_| ())?;
        if !self.verify_bytes(format!("{header}.{claims}").as_bytes(), &signature) {
            return Err(());
        }
        let jwt_header: JwtHeader =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(header).map_err(|_| ())?)
                .map_err(|_| ())?;
        if jwt_header.alg != "HS256" || jwt_header.typ != "at+jwt" {
            return Err(());
        }
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(claims).map_err(|_| ())?).map_err(|_| ())
    }

    fn sign_bytes(&self, value: &[u8]) -> Vec<u8> {
        let mut mac =
            HmacSha256::new_from_slice(&self.signing_secret).expect("HMAC accepts any key length");
        mac.update(value);
        mac.finalize().into_bytes().to_vec()
    }

    fn verify_bytes(&self, value: &[u8], signature: &[u8]) -> bool {
        let mut mac =
            HmacSha256::new_from_slice(&self.signing_secret).expect("HMAC accepts any key length");
        mac.update(value);
        mac.verify_slice(signature).is_ok()
    }

    async fn store_once<T: Serialize>(
        &self,
        kind: &str,
        raw_token: &str,
        value: &T,
        ttl_secs: u64,
    ) -> Result<(), OAuthProtocolError> {
        let key = redis_key(kind, raw_token);
        let encoded = serde_json::to_string(value).map_err(OAuthProtocolError::server_error)?;
        let mut connection = self
            .redis
            .get_multiplexed_async_connection()
            .await
            .map_err(OAuthProtocolError::temporarily_unavailable)?;
        let stored: Option<String> = redis::cmd("SET")
            .arg(key)
            .arg(encoded)
            .arg("EX")
            .arg(ttl_secs)
            .arg("NX")
            .query_async(&mut connection)
            .await
            .map_err(OAuthProtocolError::temporarily_unavailable)?;
        if stored.as_deref() != Some("OK") {
            return Err(OAuthProtocolError::server_error(
                "failed to allocate a unique OAuth grant",
            ));
        }
        Ok(())
    }

    async fn take_once<T: DeserializeOwned>(
        &self,
        kind: &str,
        raw_token: &str,
    ) -> Result<T, OAuthProtocolError> {
        let key = redis_key(kind, raw_token);
        let mut connection = self
            .redis
            .get_multiplexed_async_connection()
            .await
            .map_err(OAuthProtocolError::temporarily_unavailable)?;
        let encoded: Option<String> = redis::cmd("GETDEL")
            .arg(key)
            .query_async(&mut connection)
            .await
            .map_err(OAuthProtocolError::temporarily_unavailable)?;
        let encoded = encoded.ok_or_else(|| {
            OAuthProtocolError::invalid_grant("grant is invalid, expired, or already used")
        })?;
        serde_json::from_str(&encoded)
            .map_err(|_| OAuthProtocolError::invalid_grant("stored grant is invalid"))
    }
}

#[derive(Debug)]
struct OAuthProtocolError {
    error: &'static str,
    description: String,
    status: StatusCode,
}

impl OAuthProtocolError {
    fn new(error: &'static str, description: impl Into<String>) -> Self {
        Self {
            error,
            description: description.into(),
            status: StatusCode::BAD_REQUEST,
        }
    }

    fn invalid_request(description: impl Into<String>) -> Self {
        Self::new("invalid_request", description)
    }

    fn invalid_grant(description: impl Into<String>) -> Self {
        Self::new("invalid_grant", description)
    }

    fn invalid_client(description: impl Into<String>) -> Self {
        let mut error = Self::new("invalid_client", description);
        error.status = StatusCode::UNAUTHORIZED;
        error
    }

    fn invalid_target(description: impl Into<String>) -> Self {
        Self::new("invalid_target", description)
    }

    fn server_error(error: impl std::fmt::Display) -> Self {
        Self {
            error: "server_error",
            description: error.to_string(),
            status: StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn temporarily_unavailable(error: impl std::fmt::Display) -> Self {
        tracing::warn!(error = %error, "OAuth state store unavailable");
        Self {
            error: "temporarily_unavailable",
            description: "OAuth state store is temporarily unavailable".to_string(),
            status: StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    fn into_json_response(self) -> Response {
        no_store_json(
            self.status,
            json!({ "error": self.error, "error_description": self.description }),
        )
    }
}

pub async fn protected_resource_metadata(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let Some(oauth) = state.oauth.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match oauth.protected_resource_metadata(&headers) {
        Ok(value) => no_store_json(StatusCode::OK, value),
        Err(_) => json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "unrecognized public origin" }),
        ),
    }
}

pub async fn authorization_server_metadata(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let Some(oauth) = state.oauth.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match oauth.authorization_server_metadata(&headers) {
        Ok(value) => no_store_json(StatusCode::OK, value),
        Err(_) => json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "unrecognized public origin" }),
        ),
    }
}

pub async fn register(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ClientRegistrationRequest>,
) -> Response {
    let Some(oauth) = state.oauth.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match oauth.register_client(&headers, request) {
        Ok(value) => no_store_json(StatusCode::CREATED, value),
        Err(error) => error.into_json_response(),
    }
}

pub async fn authorize_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(request): Query<AuthorizationRequest>,
) -> Response {
    let Some(oauth) = state.oauth.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match oauth.authorization_page(&headers, request) {
        Ok(response) => response,
        Err(error) => error.into_json_response(),
    }
}

pub async fn authorize_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(decision): Form<AuthorizationDecision>,
) -> Response {
    let Some(oauth) = state.oauth.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match oauth.authorization_decision(&headers, decision).await {
        Ok(response) => response,
        Err(error) => error.into_json_response(),
    }
}

pub async fn token(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(request): Form<TokenRequest>,
) -> Response {
    let Some(oauth) = state.oauth.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    oauth.token(&headers, request).await
}

fn parse_public_base_url(value: &str) -> Result<Url, String> {
    let url = Url::parse(value).map_err(|error| format!("invalid public base URL: {error}"))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() == "/"
        || url.path().ends_with('/')
    {
        return Err(
            "public base URLs must be HTTPS MCP endpoint URLs with a path and no query, fragment, credentials, or trailing slash"
                .to_string(),
        );
    }
    Ok(url)
}

fn authority(url: &Url) -> String {
    match url.port() {
        Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
        None => url.host_str().unwrap_or_default().to_string(),
    }
}

fn validate_registration_request(
    request: &ClientRegistrationRequest,
) -> Result<(), OAuthProtocolError> {
    if request.redirect_uris.is_empty() || request.redirect_uris.len() > MAX_REDIRECT_URIS {
        return Err(OAuthProtocolError::invalid_request(
            "redirect_uris must contain between 1 and 10 entries",
        ));
    }
    for redirect in &request.redirect_uris {
        validate_redirect_uri(redirect)?;
    }
    if !request.grant_types.is_empty()
        && request
            .grant_types
            .iter()
            .any(|grant| grant != "authorization_code" && grant != "refresh_token")
    {
        return Err(OAuthProtocolError::invalid_request(
            "unsupported grant_types value",
        ));
    }
    if !request.response_types.is_empty()
        && request
            .response_types
            .iter()
            .any(|response| response != "code")
    {
        return Err(OAuthProtocolError::invalid_request(
            "unsupported response_types value",
        ));
    }
    if request
        .token_endpoint_auth_method
        .as_deref()
        .is_some_and(|method| method != "none")
    {
        return Err(OAuthProtocolError::invalid_request(
            "only token_endpoint_auth_method=none is supported",
        ));
    }
    Ok(())
}

fn validate_redirect_uri(value: &str) -> Result<(), OAuthProtocolError> {
    if value.chars().count() > MAX_REDIRECT_URI_CHARS {
        return Err(OAuthProtocolError::invalid_request(
            "redirect_uri exceeds 2048 characters",
        ));
    }
    let url = Url::parse(value)
        .map_err(|_| OAuthProtocolError::invalid_request("redirect_uri is not a valid URI"))?;
    let loopback = url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if (url.scheme() != "https" && !(url.scheme() == "http" && loopback))
        || url.host_str().is_none()
        || url.username() != ""
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(OAuthProtocolError::invalid_request(
            "redirect_uri must use HTTPS, except for an HTTP loopback redirect, and must not contain credentials or a fragment",
        ));
    }
    Ok(())
}

fn normalize_scope(scope: Option<&str>) -> Result<String, OAuthProtocolError> {
    let requested = match scope.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => value
            .split_whitespace()
            .map(str::to_string)
            .collect::<BTreeSet<_>>(),
        None => RESOURCE_SCOPES
            .iter()
            .map(|value| value.to_string())
            .collect(),
    };
    if requested
        .iter()
        .any(|value| !ALL_SCOPES.contains(&value.as_str()))
    {
        return Err(OAuthProtocolError::new(
            "invalid_scope",
            "one or more requested scopes are unsupported",
        ));
    }
    if !requested.contains(SCOPE_MCP_TOOLS) {
        return Err(OAuthProtocolError::new(
            "invalid_scope",
            "mcp:tools is required",
        ));
    }
    Ok(requested.into_iter().collect::<Vec<_>>().join(" "))
}

fn valid_pkce_value(value: &str) -> bool {
    (43..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~'))
}

fn pkce_matches(verifier: &str, expected_challenge: &str) -> bool {
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    challenge
        .as_bytes()
        .ct_eq(expected_challenge.as_bytes())
        .into()
}

fn random_token(bytes: usize) -> String {
    let mut value = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut value);
    URL_SAFE_NO_PAD.encode(value)
}

fn short_hash(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    URL_SAFE_NO_PAD.encode(&digest[..12])
}

fn redis_key(kind: &str, raw_token: &str) -> String {
    format!("{REDIS_PREFIX}:{kind}:{}", short_hash(raw_token))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn env_u64(name: &str, default: u64, max: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .map(|value| value.min(max))
        .unwrap_or(default)
}

fn required(value: Option<String>, name: &'static str) -> Result<String, OAuthProtocolError> {
    value
        .filter(|value| !value.is_empty())
        .ok_or_else(|| OAuthProtocolError::invalid_request(format!("{name} is required")))
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn no_store_json(status: StatusCode, body: Value) -> Response {
    let mut response = json_response(status, body);
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    response
}

fn html_response(status: StatusCode, body: String) -> Response {
    let mut response = (status, Html(body)).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'",
        ),
    );
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

fn redirect_with_oauth_result(
    redirect_uri: &str,
    values: &[(&str, &str)],
    state: Option<&str>,
) -> Result<Response, OAuthProtocolError> {
    let mut url = Url::parse(redirect_uri)
        .map_err(|_| OAuthProtocolError::invalid_request("invalid registered redirect_uri"))?;
    {
        let mut pairs = url.query_pairs_mut();
        for (key, value) in values {
            pairs.append_pair(key, value);
        }
        if let Some(state) = state {
            pairs.append_pair("state", state);
        }
    }
    let location = HeaderValue::from_str(url.as_str())
        .map_err(|_| OAuthProtocolError::invalid_request("invalid redirect location"))?;
    let mut response = StatusCode::SEE_OTHER.into_response();
    response.headers_mut().insert(header::LOCATION, location);
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> OAuthService {
        OAuthService::for_test(
            "https://browser.example.test/browser-mcp",
            "this-is-a-test-signing-secret-with-more-than-32-bytes",
            "this-is-a-test-operator-secret",
        )
    }

    fn headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::HOST,
            HeaderValue::from_static("browser.example.test"),
        );
        headers
    }

    #[test]
    fn metadata_advertises_rfc9728_and_pkce() {
        let service = service();
        assert_eq!(
            OAuthService::resource_metadata_url(&service.public_base_urls[0]),
            "https://browser.example.test/.well-known/oauth-protected-resource/browser-mcp"
        );
        let resource = service.protected_resource_metadata(&headers()).unwrap();
        assert_eq!(
            resource["resource"],
            "https://browser.example.test/browser-mcp"
        );
        assert_eq!(
            resource["authorization_servers"][0],
            "https://browser.example.test/browser-mcp"
        );
        let server = service.authorization_server_metadata(&headers()).unwrap();
        assert_eq!(server["code_challenge_methods_supported"][0], "S256");
        assert_eq!(server["token_endpoint_auth_methods_supported"][0], "none");
        assert!(server["scopes_supported"]
            .as_array()
            .unwrap()
            .contains(&json!("offline_access")));
    }

    #[test]
    fn dynamic_client_id_is_signed_and_redirect_bound() {
        let service = service();
        let response = service
            .register_client(
                &headers(),
                ClientRegistrationRequest {
                    redirect_uris: vec!["https://chatgpt.com/connector/oauth/callback".to_string()],
                    client_name: Some("ChatGPT".to_string()),
                    grant_types: vec![
                        "authorization_code".to_string(),
                        "refresh_token".to_string(),
                    ],
                    response_types: vec!["code".to_string()],
                    token_endpoint_auth_method: Some("none".to_string()),
                },
            )
            .unwrap();
        let client: RegisteredClient = service
            .unseal("client", response["client_id"].as_str().unwrap())
            .unwrap();
        assert_eq!(client.client_name, "ChatGPT");
        assert_eq!(
            client.redirect_uris,
            vec!["https://chatgpt.com/connector/oauth/callback"]
        );
    }

    #[test]
    fn pkce_s256_matches_only_the_original_verifier() {
        let verifier = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~";
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        assert!(pkce_matches(verifier, &challenge));
        assert!(!pkce_matches(
            "different-verifier-that-is-long-enough-to-be-valid-123456",
            &challenge
        ));
    }

    #[test]
    fn access_tokens_are_signed_scoped_and_audience_bound() {
        let service = service();
        let issued_at = now();
        let token = service
            .sign_access_token(&AccessTokenClaims {
                iss: "https://browser.example.test/browser-mcp".to_string(),
                sub: "operator:test".to_string(),
                aud: "https://browser.example.test/browser-mcp".to_string(),
                client_id: "client".to_string(),
                scope: "browser:read mcp:tools".to_string(),
                iat: issued_at,
                nbf: issued_at,
                exp: issued_at + 900,
                jti: "jti".to_string(),
            })
            .unwrap();
        let mut request_headers = headers();
        request_headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        assert!(service
            .authenticate(&request_headers, &[SCOPE_MCP_TOOLS, SCOPE_BROWSER_READ])
            .is_ok());
        assert!(matches!(
            service.authenticate(&request_headers, &[SCOPE_BROWSER_ACT]),
            Err(AccessError::InsufficientScope)
        ));
    }

    #[test]
    fn redirect_uris_reject_fragments_and_non_loopback_http() {
        assert!(validate_redirect_uri("https://chatgpt.com/callback").is_ok());
        assert!(validate_redirect_uri("http://127.0.0.1:8181/callback").is_ok());
        assert!(validate_redirect_uri("http://example.com/callback").is_err());
        assert!(validate_redirect_uri("https://chatgpt.com/callback#fragment").is_err());
        assert!(validate_redirect_uri(&format!(
            "https://chatgpt.com/{}",
            "x".repeat(MAX_REDIRECT_URI_CHARS)
        ))
        .is_err());
    }
}
