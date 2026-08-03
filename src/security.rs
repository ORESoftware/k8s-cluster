//! Request-security gates for the customer plane: CSRF/idempotency checks,
//! the request-security and MFA-assurance middleware, sensitive-response
//! header hardening, and Supabase auth-error rendering. Extracted from main.rs.

use super::*;

/// Map a Supabase auth failure onto a rendered login response. `Rejected` is the
/// user's fault (bad/expired code) and re-renders the given page with the message
/// at 401; transport/parse failures surface as a 503 dependency error.
pub(crate) fn supabase_auth_error_response(
    error: SupabaseAuthError,
    retry_page: Response,
    mut retry_status: StatusCode,
) -> Response {
    match error {
        SupabaseAuthError::Invalid(reason) => {
            tracing::debug!(reason, "rejected malformed passwordless input");
            let mut page = retry_page;
            *page.status_mut() = StatusCode::BAD_REQUEST;
            page
        }
        SupabaseAuthError::Rejected(detail) => {
            tracing::info!(detail, "supabase rejected passwordless/mfa request");
            let mut page = retry_page;
            if retry_status == StatusCode::OK {
                retry_status = StatusCode::UNAUTHORIZED;
            }
            *page.status_mut() = retry_status;
            page
        }
        SupabaseAuthError::Unavailable(detail) => {
            dependency_error("supabase", "supabase_auth_unavailable", detail)
        }
    }
}

pub(crate) fn request_security_error(error: RequestSecurityError) -> Response {
    tracing::warn!(reason = error.code(), "rejected untrusted customer request");
    (
        StatusCode::FORBIDDEN,
        Json(json!({
            "ok": false,
            "error": "customer_request_rejected",
            "reason": error.code()
        })),
    )
        .into_response()
}

pub(crate) fn customer_csrf_token(config: &AppConfig, customer: &CustomerCtx) -> String {
    config.request_security.csrf_token(customer.csrf_binding())
}

pub(crate) fn require_form_security(
    headers: &HeaderMap,
    config: &AppConfig,
    customer: &CustomerCtx,
    provided_csrf: &str,
) -> Result<(), RequestSecurityError> {
    config.request_security.require_same_origin(headers)?;
    config
        .request_security
        .verify_csrf_token(customer.csrf_binding(), provided_csrf)
}

pub(crate) fn require_api_write_security(
    headers: &HeaderMap,
    config: &AppConfig,
    customer: &CustomerCtx,
) -> Result<(), RequestSecurityError> {
    if customer.is_browser_session() {
        config.request_security.require_same_origin(headers)?;
        let provided = headers
            .get(CUSTOMER_CSRF_HEADER)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        config
            .request_security
            .verify_csrf_token(customer.csrf_binding(), provided)
    } else {
        config.request_security.require_api_host(headers)
    }
}

pub(crate) async fn request_security_gate(
    State(security): State<RequestSecurity>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    let method = request.method();
    let browser_surface =
        path.starts_with("/login") || path == "/logout" || path.starts_with("/app");
    let customer_api = path.starts_with("/api/customer");
    let exact_origin_required = path == CUSTOMER_WS_PATH
        || (browser_surface && !matches!(*method, Method::GET | Method::HEAD));
    let result = if exact_origin_required {
        security.require_same_origin(request.headers())
    } else if browser_surface || customer_api {
        security.require_api_host(request.headers())
    } else {
        Ok(())
    };
    if let Err(error) = result {
        return request_security_error(error);
    }
    next.run(request).await
}

/// Apply the AAL invariant once at the router boundary, rather than relying on
/// each handler (or a future route) to remember it. Login and logout remain
/// reachable: an `aal1` browser must be able to complete MFA or clear a stale
/// cookie. The root is protected only when this host serves the customer app;
/// the public marketing host remains public.
pub(crate) async fn customer_mfa_assurance_gate(
    State(config): State<AppConfig>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    // Preserve route-method semantics: the catch-up endpoint is GET-only, and
    // an unsupported method must remain a 405 rather than being transformed
    // into an authentication response by this outer middleware.
    let unsupported_sync_method = path.starts_with("/api/customer/sync/")
        && !matches!(*request.method(), Method::GET | Method::HEAD);
    let customer_root = path == "/" && should_serve_customer_app(&config, request.headers());
    let protected = path.starts_with("/app") || path.starts_with("/api/customer") || customer_root;
    if protected && !unsupported_sync_method {
        if let Err(response) = config.authenticate_mfa_aware(request.headers()).await {
            // Browser pages previously turn an absent/invalid session into the
            // login flow. Preserve that UX for the new aal1 backstop as well;
            // JSON customer APIs retain an explicit 401 for programmatic callers.
            if response.status() == StatusCode::UNAUTHORIZED
                && (path.starts_with("/app") || customer_root)
            {
                return (StatusCode::SEE_OTHER, [(header::LOCATION, "/login")]).into_response();
            }
            return response;
        }
    }
    next.run(request).await
}

/// Harden a customer-sensitive response: never cache it (it carries the user's
/// email, org ids, and CSRF token) and pin the strict portal CSP. Applied both
/// by the path-based middleware and directly by the portal renderer, because the
/// authenticated dashboard is reachable at `/` (app host) as well as `/app*`.
pub(crate) fn apply_sensitive_response_headers(headers: &mut HeaderMap) {
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    // The vendored htmx runtime injects one fixed indicator stylesheet. Authorize
    // only that exact stylesheet digest rather than weakening the portal with
    // `unsafe-inline`; a vendored-asset change must update both this hash and the
    // real-browser assertion before CI can pass.
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'; object-src 'none'; connect-src 'self'; img-src 'self' data:; style-src 'self' 'sha256-faU7yAF8NxuMTNEwVmBz+VcYeIoBQ2EMHW3WaVxCvnk='",
        ),
    );
    // `same-origin`, NOT `no-referrer`: under `no-referrer` a browser
    // serializes the Origin of any non-GET request (form POST, SPA fetch) as
    // `null`, so `require_same_origin` / `require_api_host` would reject every
    // real-browser mutation while hand-crafted clients that set Origin
    // themselves pass — the inversion of the intent. `same-origin` still never
    // leaks the referrer cross-origin and keeps Origin intact for the gate.
    // Proven by the real-Chromium journeys in fiducia-e2e (npm run test:browser).
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
}

/// Host/mode inputs the outermost header middleware needs to recognize when `/`
/// is serving the authenticated portal (rather than the public marketing index).
#[derive(Clone)]
pub(crate) struct SensitiveHeaderContext {
    pub(crate) customer_app_host: String,
    pub(crate) customer_site_mode: bool,
}

pub(crate) async fn security_headers(
    State(ctx): State<SensitiveHeaderContext>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    // `/` serves the customer dashboard on the app host (or in customer-site mode),
    // carrying the same email/org/CSRF material as `/app`, but the path prefixes
    // below don't catch it — classify it as sensitive so it is never cacheable.
    let root_is_portal = path == "/"
        && host_serves_customer_app(
            request.headers(),
            &ctx.customer_app_host,
            ctx.customer_site_mode,
        );
    let sensitive = root_is_portal
        || path.starts_with("/login")
        || path == "/logout"
        || path.starts_with("/app")
        || path.starts_with("/api/customer");
    let mut response = next.run(request).await;
    if sensitive {
        apply_sensitive_response_headers(response.headers_mut());
    }
    response
}
