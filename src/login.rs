//! Customer authentication surface: the Supabase password/OTP/MFA login
//! flow, its throttling and CSRF/session cookies, logout, the standalone MFA
//! settings page, and all the maud markup for those pages. Extracted from
//! main.rs; `use super::*` inherits the crate-root config, security helpers,
//! and Supabase auth client these handlers drive.
#![allow(clippy::too_many_lines)]

use super::*;

#[derive(Debug, Deserialize)]
pub(crate) struct CustomerLoginForm {
    csrf_token: String,
    email: String,
    password: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SupabasePasswordSession {
    access_token: String,
}

pub(crate) async fn htmx_js() -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        HTMX_JS,
    )
}

pub(crate) async fn customer_css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        CUSTOMER_CSS,
    )
}

pub(crate) async fn customer_login(State(config): State<AppConfig>) -> Response {
    customer_login_page(&config, None)
}

pub(crate) async fn customer_login_submit(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Form(form): Form<CustomerLoginForm>,
) -> Response {
    if let Err(error) = require_login_security(&headers, &config, &form.csrf_token) {
        return request_security_error(error);
    }
    let email = form.email.trim();
    if email.is_empty() || form.password.is_empty() {
        let mut response = customer_login_page(&config, Some("Email and password are required."));
        *response.status_mut() = StatusCode::BAD_REQUEST;
        return response;
    }
    // Bound credential stuffing against one account before spending an upstream
    // round trip on it.
    let password_budget = throttle::check(throttle::Bucket::PasswordPerIdentifier, email);
    if !password_budget.allowed {
        return throttled_response(
            customer_login_page(&config, Some(THROTTLE_MESSAGE)),
            password_budget.retry_after_secs,
        );
    }
    let (Some(supabase_url), Some(publishable_key)) = (
        config.supabase_url.as_deref(),
        config.supabase_publishable_key.as_deref(),
    ) else {
        return dependency_error(
            "supabase",
            "customer_login_not_configured",
            "SUPABASE_URL and SUPABASE_PUBLISHABLE_KEY are required",
        );
    };

    let response = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(client) => {
            client
                .post(format!(
                    "{}/auth/v1/token?grant_type=password",
                    supabase_url.trim_end_matches('/')
                ))
                .header("apikey", publishable_key)
                .json(&json!({ "email": email, "password": form.password }))
                .send()
                .await
        }
        Err(error) => return dependency_error("supabase", "supabase_login_failed", error),
    };
    let response = match response {
        Ok(response) if response.status().is_success() => response,
        Ok(_) => {
            let mut response =
                customer_login_page(&config, Some("Supabase rejected those credentials."));
            *response.status_mut() = StatusCode::UNAUTHORIZED;
            return response;
        }
        Err(error) => return dependency_error("supabase", "supabase_login_failed", error),
    };
    let session = match response.json::<SupabasePasswordSession>().await {
        Ok(session) => session,
        Err(error) => return dependency_error("supabase", "supabase_login_failed", error),
    };

    // Fail closed on MFA before issuing any app cookie: Supabase's password grant
    // returns an aal1 token even for MFA-enrolled accounts, so the password form
    // must run the SAME factor check as the OTP path (/login/verify) — otherwise a
    // verified TOTP factor is trivially bypassed by choosing the password form. A
    // factor-lookup outage is a 503, never a silent single-factor admit.
    let Some(supabase) = config.supabase_auth() else {
        return dependency_error(
            "supabase",
            "customer_login_not_configured",
            "SUPABASE_URL and SUPABASE_PUBLISHABLE_KEY are required",
        );
    };
    match supabase.list_factors(&session.access_token).await {
        Ok(factors) => match required_totp_factor(&factors) {
            Some(factor_id) => {
                begin_mfa_step_up(&config, &supabase, &session.access_token, &factor_id).await
            }
            None => finalize_customer_login(&config, &session.access_token).await,
        },
        Err(error) => dependency_error("supabase", "mfa_state_unavailable", error),
    }
}

/// Render an unauthenticated login-flow page and bind it to a fresh login-CSRF
/// nonce cookie. `build` receives the HMAC token to embed in its form(s); the
/// subsequent POST is validated by [`require_login_security`]. Every pre-session
/// form page (login, OTP entry, MFA step-up) goes through here so they all share
/// one CSRF contract.
pub(crate) fn login_flow_page(config: &AppConfig, build: impl FnOnce(&str) -> Markup) -> Response {
    let nonce = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let token = config
        .request_security
        .csrf_token(&format!("login\0{nonce}"));
    let mut response = build(&token).into_response();
    append_set_cookie(&mut response, &make_customer_login_csrf_cookie(&nonce));
    response
}

pub(crate) fn customer_login_page(config: &AppConfig, message: Option<&str>) -> Response {
    login_flow_page(config, |token| customer_login_markup(message, token))
}

/// The client identity for per-client throttle buckets. See
/// [`throttle::client_key`] for why only the LAST `X-Forwarded-For` hop is
/// trustworthy behind the nginx gateway.
pub(crate) fn throttle_client_key(headers: &HeaderMap) -> String {
    throttle::client_key(
        headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok()),
    )
}

/// Refuse an over-budget attempt: 429 + `Retry-After`, re-rendering `page` so the
/// user still gets a usable form.
///
/// The message is deliberately generic and identical whether or not the
/// identifier exists — a throttle response must not become the account-existence
/// oracle that the rest of this flow is careful to avoid.
pub(crate) fn throttled_response(page: Response, retry_after_secs: u64) -> Response {
    let mut response = page;
    *response.status_mut() = StatusCode::TOO_MANY_REQUESTS;
    if let Ok(value) = HeaderValue::from_str(&retry_after_secs.to_string()) {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

pub(crate) const THROTTLE_MESSAGE: &str =
    "Too many attempts. Wait a few minutes before trying again.";

pub(crate) fn require_login_security(
    headers: &HeaderMap,
    config: &AppConfig,
    provided_csrf: &str,
) -> Result<(), RequestSecurityError> {
    config.request_security.require_same_origin(headers)?;
    let nonce = cookie_value(headers, CUSTOMER_LOGIN_CSRF_COOKIE)
        .ok_or(RequestSecurityError::InvalidCsrfToken)?;
    config
        .request_security
        .verify_csrf_token(&format!("login\0{nonce}"), provided_csrf)
}

pub(crate) fn append_set_cookie(response: &mut Response, cookie: &str) {
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_str(cookie).expect("server-generated cookie is a valid header value"),
    );
}

pub(crate) fn explicitly_enabled(value: Option<&str>) -> bool {
    value.is_some_and(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true"))
}

pub(crate) const fn cookie_secure_suffix_for(
    release_hardened: bool,
    insecure_http_explicitly_enabled: bool,
) -> &'static str {
    if release_hardened || !insecure_http_explicitly_enabled {
        "; Secure"
    } else {
        ""
    }
}

#[cfg(debug_assertions)]
pub(crate) fn cookie_secure_suffix() -> &'static str {
    cookie_secure_suffix_for(
        false,
        explicitly_enabled(std::env::var("FIDUCIA_INSECURE_COOKIES").ok().as_deref()),
    )
}

#[cfg(not(debug_assertions))]
pub(crate) fn cookie_secure_suffix() -> &'static str {
    let insecure_requested =
        explicitly_enabled(std::env::var("FIDUCIA_INSECURE_COOKIES").ok().as_deref());
    if insecure_requested {
        tracing::error!(
            "FIDUCIA_INSECURE_COOKIES is set but IGNORED: release builds always emit Secure cookies"
        );
    }
    cookie_secure_suffix_for(true, insecure_requested)
}

pub(crate) fn make_customer_login_csrf_cookie(nonce: &str) -> String {
    format!(
        "{CUSTOMER_LOGIN_CSRF_COOKIE}={nonce}; Path=/; HttpOnly; SameSite=Strict; Max-Age=600{}",
        cookie_secure_suffix()
    )
}

pub(crate) fn clear_customer_login_csrf_cookie() -> String {
    format!(
        "{CUSTOMER_LOGIN_CSRF_COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{}",
        cookie_secure_suffix()
    )
}

pub(crate) fn make_customer_session_cookie(token: &str) -> String {
    format!(
        "{CUSTOMER_SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age=3600{}",
        cookie_secure_suffix()
    )
}

pub(crate) fn clear_customer_session_cookie() -> String {
    format!(
        "{CUSTOMER_SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{}",
        cookie_secure_suffix()
    )
}

pub(crate) fn make_customer_mfa_pending_cookie(token: &str) -> String {
    format!(
        "{CUSTOMER_MFA_PENDING_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={MFA_PENDING_MAX_AGE_SECS}{}",
        cookie_secure_suffix()
    )
}

pub(crate) fn clear_customer_mfa_pending_cookie() -> String {
    format!(
        "{CUSTOMER_MFA_PENDING_COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{}",
        cookie_secure_suffix()
    )
}

#[derive(Debug, Deserialize)]
pub(crate) struct CustomerLogoutForm {
    csrf_token: String,
}

pub(crate) async fn customer_logout(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Form(form): Form<CustomerLogoutForm>,
) -> Response {
    let customer = match config.authenticator.authenticate(&headers).await {
        Ok(customer) => customer,
        Err(response) => return response,
    };
    if let Err(error) = require_form_security(&headers, &config, &customer, &form.csrf_token) {
        return request_security_error(error);
    }
    let mut response = (StatusCode::SEE_OTHER, [(header::LOCATION, "/login")]).into_response();
    append_set_cookie(&mut response, &clear_customer_session_cookie());
    // Clear the transient login cookies too. A user who abandons a half-finished
    // step-up and then signs out would otherwise leave a live pre-2FA Supabase
    // bearer in the browser for the rest of its 300s window — on a shared
    // machine that is a residual credential, and "sign out" must mean all of it.
    append_set_cookie(&mut response, &clear_customer_mfa_pending_cookie());
    append_set_cookie(&mut response, &clear_customer_login_csrf_cookie());
    response
}

/// Shared chrome for every unauthenticated auth page (login, OTP entry, MFA
/// step-up). Keeps one `head`/shell so the flows are visually one surface.
pub(crate) fn auth_page_shell(title: &str, inner: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) }
                link rel="stylesheet" href="/assets/customer.css";
                script src="/assets/htmx.min.js" defer {}
            }
            body {
                main class="auth-shell" {
                    section class="auth-card" {
                        (inner)
                    }
                }
            }
        }
    }
}

pub(crate) fn customer_login_markup(message: Option<&str>, csrf_token: &str) -> Markup {
    auth_page_shell(
        "Sign in · Fiducia Customer",
        html! {
            p class="eyebrow" { "Customer application" }
            h1 { "Sign in to Fiducia" }
            p class="muted" { "Supabase authenticates you; fiducia-auth verifies the resulting identity and organization membership." }
            @if let Some(message) = message {
                p class="auth-message" role="alert" { (message) }
            }

            // Password grant (unchanged surface).
            form method="post" action="/login" hx-post="/login" hx-target="body" hx-swap="outerHTML" {
                h2 { "Email & password" }
                input type="hidden" name="csrf_token" value=(csrf_token);
                label for="email" { "Email" }
                input id="email" name="email" type="email" autocomplete="email" required;
                label for="password" { "Password" }
                input id="password" name="password" type="password" autocomplete="current-password" required;
                button type="submit" { "Sign in" }
            }

            // Passwordless email — 6-digit code (also self-signup).
            //
            // Copy says "code", not "magic link", because the link is not wired:
            // `send_otp` passes no `email_redirect_to` and this router has no
            // callback route, so the link in Supabase's mail lands on the
            // project's Site URL — outside this app's __Host-/HttpOnly cookie
            // boundary. Promising one-tap sign-in would be promising a flow that
            // dead-ends. Wiring it properly (redirect_to + a callback that
            // exchanges token_hash and runs the same step-up gate) is follow-up.
            form method="post" action="/login/otp" hx-post="/login/otp" hx-target="body" hx-swap="outerHTML" {
                h2 { "Email a sign-in code" }
                p class="muted" { "We email you a 6-digit code. New here? This also creates your account." }
                input type="hidden" name="csrf_token" value=(csrf_token);
                input type="hidden" name="method" value="email";
                label for="magic-email" { "Email" }
                input id="magic-email" name="identifier" type="email" autocomplete="email" required;
                button type="submit" { "Email me a link" }
            }

            // Passwordless phone — SMS one-time passcode.
            form method="post" action="/login/otp" hx-post="/login/otp" hx-target="body" hx-swap="outerHTML" {
                h2 { "Phone code" }
                p class="muted" { "We text a 6-digit code to your phone. Use international format, e.g. +14155550123." }
                input type="hidden" name="csrf_token" value=(csrf_token);
                input type="hidden" name="method" value="phone";
                label for="otp-phone" { "Phone" }
                input id="otp-phone" name="identifier" type="tel" autocomplete="tel" inputmode="tel"
                    placeholder="+14155550123" required;
                button type="submit" { "Text me a code" }
            }

            p class="muted" { "Accounts with an authenticator app will be asked for a 6-digit code after this step." }
            p class="muted" { "Operator accounts use the separate admin application and cookie boundary." }
        },
    )
}

/// OTP-entry page shown after a code is dispatched. Carries the channel +
/// identifier forward so `/login/verify` knows how to redeem the code.
pub(crate) fn otp_verify_markup(
    channel: OtpChannel,
    identifier: &str,
    csrf_token: &str,
    message: Option<&str>,
) -> Markup {
    let heading = match channel {
        OtpChannel::Email => "Check your email",
        OtpChannel::Phone => "Check your phone",
    };
    let blurb = match channel {
        OtpChannel::Email => "We emailed you a 6-digit code. Enter it below.",
        OtpChannel::Phone => "We texted a 6-digit code to your phone. Enter it below.",
    };
    auth_page_shell(
        "Enter your code · Fiducia Customer",
        html! {
            p class="eyebrow" { "Customer application" }
            h1 { (heading) }
            p class="muted" { (blurb) }
            p class="muted" { "Sending to " strong { (identifier) } "." }
            @if let Some(message) = message {
                p class="auth-message" role="alert" { (message) }
            }
            form method="post" action="/login/verify" hx-post="/login/verify" hx-target="body" hx-swap="outerHTML" {
                input type="hidden" name="csrf_token" value=(csrf_token);
                input type="hidden" name="method" value=(channel.field());
                input type="hidden" name="identifier" value=(identifier);
                label for="otp-code" { "6-digit code" }
                input id="otp-code" name="token" type="text" inputmode="numeric" autocomplete="one-time-code"
                    pattern="[0-9]*" minlength="6" maxlength="8" required;
                button type="submit" { "Verify & continue" }
            }
            form method="post" action="/login/otp" hx-post="/login/otp" hx-target="body" hx-swap="outerHTML" {
                input type="hidden" name="csrf_token" value=(csrf_token);
                input type="hidden" name="method" value=(channel.field());
                input type="hidden" name="identifier" value=(identifier);
                button type="submit" class="link-button" { "Resend code" }
            }
            a href="/login" { "Start over" }
        },
    )
}

/// TOTP step-up page. The primary factor already succeeded; the account has a
/// verified authenticator, so we require its current 6-digit code before issuing
/// the app session cookie. `factor_id`/`challenge_id` ride hidden fields; the
/// aal1 token rides the short-lived pending cookie.
pub(crate) fn mfa_challenge_markup(
    factor_id: &str,
    challenge_id: &str,
    csrf_token: &str,
    message: Option<&str>,
) -> Markup {
    auth_page_shell(
        "Two-factor verification · Fiducia Customer",
        html! {
            p class="eyebrow" { "Two-factor authentication" }
            h1 { "Enter your authenticator code" }
            p class="muted" { "Open your authenticator app (Authy, Google Authenticator, 1Password…) and enter the current 6-digit code for Fiducia." }
            @if let Some(message) = message {
                p class="auth-message" role="alert" { (message) }
            }
            form method="post" action="/login/mfa" hx-post="/login/mfa" hx-target="body" hx-swap="outerHTML" {
                input type="hidden" name="csrf_token" value=(csrf_token);
                input type="hidden" name="factor_id" value=(factor_id);
                input type="hidden" name="challenge_id" value=(challenge_id);
                label for="mfa-code" { "Authenticator code" }
                input id="mfa-code" name="code" type="text" inputmode="numeric" autocomplete="one-time-code"
                    pattern="[0-9]*" minlength="6" maxlength="8" required;
                button type="submit" { "Verify" }
            }
            a href="/login" { "Cancel and sign in again" }
        },
    )
}

#[derive(Debug, Deserialize)]
pub(crate) struct OtpRequestForm {
    csrf_token: String,
    /// "email" or "phone".
    method: String,
    identifier: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OtpVerifyForm {
    csrf_token: String,
    method: String,
    identifier: String,
    token: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MfaStepUpForm {
    csrf_token: String,
    factor_id: String,
    challenge_id: String,
    code: String,
}

pub(crate) fn parse_otp_channel(method: &str) -> Option<OtpChannel> {
    match method.trim() {
        "email" => Some(OtpChannel::Email),
        "phone" => Some(OtpChannel::Phone),
        _ => None,
    }
}

/// Complete a login once the caller is fully authenticated (either single-factor
/// or post-TOTP): re-verify the resulting Supabase token against fiducia-auth
/// (identity + org membership), then set the app session cookie and clear the
/// transient login/MFA cookies. Mirrors the password path's finalize step so all
/// entry points converge on one org-scoping check.
pub(crate) async fn finalize_customer_login(config: &AppConfig, access_token: &str) -> Response {
    let mut verify_headers = HeaderMap::new();
    let bearer = match HeaderValue::from_str(&format!("Bearer {access_token}")) {
        Ok(value) => value,
        Err(error) => return dependency_error("supabase", "supabase_login_failed", error),
    };
    verify_headers.insert(header::AUTHORIZATION, bearer);
    if let Err(response) = config.authenticator.authenticate(&verify_headers).await {
        return response;
    }
    let mut response = (StatusCode::SEE_OTHER, [(header::LOCATION, "/app")]).into_response();
    append_set_cookie(&mut response, &make_customer_session_cookie(access_token));
    append_set_cookie(&mut response, &clear_customer_login_csrf_cookie());
    append_set_cookie(&mut response, &clear_customer_mfa_pending_cookie());
    response
}

/// `POST /login/otp` — dispatch a one-time code over email or phone.
/// `should_create_user` is true so this doubles as self-service signup.
pub(crate) async fn customer_login_otp_submit(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Form(form): Form<OtpRequestForm>,
) -> Response {
    if let Err(error) = require_login_security(&headers, &config, &form.csrf_token) {
        return request_security_error(error);
    }
    let Some(channel) = parse_otp_channel(&form.method) else {
        let mut page =
            customer_login_page(&config, Some("Choose email or phone to receive a code."));
        *page.status_mut() = StatusCode::BAD_REQUEST;
        return page;
    };
    let Some(supabase) = config.supabase_auth() else {
        return dependency_error(
            "supabase",
            "customer_login_not_configured",
            "SUPABASE_URL and SUPABASE_PUBLISHABLE_KEY are required",
        );
    };
    let identifier = form.identifier.trim().to_string();
    // Two budgets, two different abuses. Per-identifier stops a victim being
    // spammed with texts/mail they never asked for; per-client caps what one
    // caller can bill us when it rotates the destination every request (SMS
    // pumping). Both are checked BEFORE `send_otp` — the cost is the dispatch.
    let identifier_budget =
        throttle::check(throttle::Bucket::OtpDispatchPerIdentifier, &identifier);
    if !identifier_budget.allowed {
        return throttled_response(
            login_flow_page(&config, |token| {
                otp_verify_markup(channel, &identifier, token, Some(THROTTLE_MESSAGE))
            }),
            identifier_budget.retry_after_secs,
        );
    }
    let client_budget = throttle::check(
        throttle::Bucket::OtpDispatchPerClient,
        &throttle_client_key(&headers),
    );
    if !client_budget.allowed {
        return throttled_response(
            customer_login_page(&config, Some(THROTTLE_MESSAGE)),
            client_budget.retry_after_secs,
        );
    }
    match supabase.send_otp(channel, &identifier, true).await {
        Ok(()) => login_flow_page(&config, |token| {
            otp_verify_markup(channel, &identifier, token, None)
        }),
        Err(error) => {
            let retry = login_flow_page(&config, |token| {
                customer_login_markup(
                    Some("We couldn't send that code. Check the address and try again."),
                    token,
                )
            });
            supabase_auth_error_response(error, retry, StatusCode::OK)
        }
    }
}

/// `POST /login/verify` — redeem the OTP, then branch on MFA: a verified
/// authenticator forces TOTP step-up before any app cookie is issued; otherwise
/// the login finalizes immediately.
pub(crate) async fn customer_login_verify_submit(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Form(form): Form<OtpVerifyForm>,
) -> Response {
    if let Err(error) = require_login_security(&headers, &config, &form.csrf_token) {
        return request_security_error(error);
    }
    let Some(channel) = parse_otp_channel(&form.method) else {
        return customer_login_page(&config, Some("Choose email or phone to receive a code."));
    };
    let Some(supabase) = config.supabase_auth() else {
        return dependency_error(
            "supabase",
            "customer_login_not_configured",
            "SUPABASE_URL and SUPABASE_PUBLISHABLE_KEY are required",
        );
    };
    let identifier = form.identifier.trim().to_string();
    // A 6-digit code is a 10^6 keyspace and the success/failure oracle here is
    // unambiguous (303 vs 401), so without an attempt cap the code is grindable.
    // Keyed on the identifier being attacked, not the caller, so rotating source
    // addresses does not buy fresh attempts against one account.
    let verify_budget = throttle::check(throttle::Bucket::OtpVerifyPerIdentifier, &identifier);
    if !verify_budget.allowed {
        return throttled_response(
            login_flow_page(&config, |token| {
                otp_verify_markup(channel, &identifier, token, Some(THROTTLE_MESSAGE))
            }),
            verify_budget.retry_after_secs,
        );
    }
    let session = match supabase.verify_otp(channel, &identifier, &form.token).await {
        Ok(session) => session,
        Err(error) => {
            let retry = login_flow_page(&config, |token| {
                otp_verify_markup(
                    channel,
                    &identifier,
                    token,
                    Some("That code didn't match or has expired. Request a new one."),
                )
            });
            return supabase_auth_error_response(error, retry, StatusCode::OK);
        }
    };

    // Fail closed: never finalize a login without knowing the account's MFA
    // state. A factor lookup outage is a 503, not a silent single-factor admit.
    match supabase.list_factors(&session.access_token).await {
        Ok(factors) => match required_totp_factor(&factors) {
            Some(factor_id) => {
                begin_mfa_step_up(&config, &supabase, &session.access_token, &factor_id).await
            }
            None => finalize_customer_login(&config, &session.access_token).await,
        },
        Err(error) => dependency_error("supabase", "mfa_state_unavailable", error),
    }
}

/// Open a TOTP challenge and render the step-up page, stashing the primary-factor
/// token in the short-lived pending cookie.
pub(crate) async fn begin_mfa_step_up(
    config: &AppConfig,
    supabase: &SupabaseAuth,
    access_token: &str,
    factor_id: &str,
) -> Response {
    let challenge = match supabase.challenge(access_token, factor_id).await {
        Ok(challenge) => challenge,
        Err(error) => return dependency_error("supabase", "mfa_challenge_failed", error),
    };
    let factor = challenge.factor_id.clone();
    let challenge_id = challenge.challenge_id.clone();
    let mut response = login_flow_page(config, |token| {
        mfa_challenge_markup(&factor, &challenge_id, token, None)
    });
    append_set_cookie(
        &mut response,
        &make_customer_mfa_pending_cookie(access_token),
    );
    response
}

/// `POST /login/mfa` — verify the authenticator code against the open challenge,
/// stepping the session up to aal2 and finalizing the login.
pub(crate) async fn customer_login_mfa_submit(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Form(form): Form<MfaStepUpForm>,
) -> Response {
    if let Err(error) = require_login_security(&headers, &config, &form.csrf_token) {
        return request_security_error(error);
    }
    let Some(pending_token) = cookie_value(&headers, CUSTOMER_MFA_PENDING_COOKIE) else {
        return customer_login_page(
            &config,
            Some("Your verification session expired. Please sign in again."),
        );
    };
    let Some(supabase) = config.supabase_auth() else {
        return dependency_error(
            "supabase",
            "customer_login_not_configured",
            "SUPABASE_URL and SUPABASE_PUBLISHABLE_KEY are required",
        );
    };
    // The challenge is deliberately kept alive across wrong codes so a user can
    // retry the current one — which equally lets an attacker holding the primary
    // factor grind the second. This cap is what makes the second factor an
    // actual barrier rather than a delegated one.
    let step_up_budget = throttle::check(
        throttle::Bucket::MfaVerifyPerClient,
        &throttle_client_key(&headers),
    );
    if !step_up_budget.allowed {
        let factor = form.factor_id.clone();
        let challenge_id = form.challenge_id.clone();
        return throttled_response(
            login_flow_page(&config, |token| {
                mfa_challenge_markup(&factor, &challenge_id, token, Some(THROTTLE_MESSAGE))
            }),
            step_up_budget.retry_after_secs,
        );
    }
    let challenge = supabase_auth::TotpChallenge {
        challenge_id: form.challenge_id.clone(),
        factor_id: form.factor_id.clone(),
    };
    match supabase
        .verify_factor(&pending_token, &challenge, &form.code)
        .await
    {
        Ok(session) => finalize_customer_login(&config, &session.access_token).await,
        Err(error) => {
            let factor = form.factor_id.clone();
            let challenge_id = form.challenge_id.clone();
            // Keep the pending cookie so the user can retry the current code.
            let retry = login_flow_page(&config, |token| {
                mfa_challenge_markup(
                    &factor,
                    &challenge_id,
                    token,
                    Some("That code didn't match. Enter the current code from your authenticator app."),
                )
            });
            supabase_auth_error_response(error, retry, StatusCode::OK)
        }
    }
}

// ── Authenticator (TOTP) enrollment on the post-login Security surface ─────────

#[derive(Debug, Deserialize)]
pub(crate) struct MfaEnrollForm {
    csrf_token: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MfaActivateForm {
    csrf_token: String,
    factor_id: String,
    code: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MfaDisableForm {
    csrf_token: String,
    factor_id: String,
    /// A current code is required even for an aal2 session: a long-lived
    /// stepped-up token alone must not be enough to remove an authenticator.
    code: String,
}

/// The raw customer Supabase access token backing the current session, read from
/// the session cookie (or a forwarded bearer). Required to manage the caller's
/// own factors via Supabase; absent → the caller isn't a browser session.
pub(crate) fn customer_access_token(headers: &HeaderMap) -> Option<String> {
    bearer_token(headers)
}

/// `GET /app/security/mfa` — authenticator management: list enrolled factors and
/// offer enrollment.
pub(crate) async fn customer_mfa_page(
    State(config): State<AppConfig>,
    headers: HeaderMap,
) -> Response {
    let customer = match config.authenticator.authenticate(&headers).await {
        Ok(customer) => customer,
        Err(response) => return response,
    };
    let Some(token) = customer_access_token(&headers) else {
        return deny_json(StatusCode::UNAUTHORIZED, "missing_customer_session");
    };
    let Some(supabase) = config.supabase_auth() else {
        return dependency_error(
            "supabase",
            "customer_login_not_configured",
            "SUPABASE_URL and SUPABASE_PUBLISHABLE_KEY are required",
        );
    };
    let factors = match supabase.list_factors(&token).await {
        Ok(factors) => factors,
        Err(error) => return dependency_error("supabase", "mfa_state_unavailable", error),
    };
    let csrf = customer_csrf_token(&config, &customer);
    mfa_settings_markup(&factors, &csrf, None).into_response()
}

/// `POST /app/security/mfa/enroll` — begin TOTP enrollment; renders the QR +
/// secret + an activation form. Nothing is active until a code is confirmed.
pub(crate) async fn customer_mfa_enroll(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Form(form): Form<MfaEnrollForm>,
) -> Response {
    let customer = match config.authenticator.authenticate(&headers).await {
        Ok(customer) => customer,
        Err(response) => return response,
    };
    if let Err(error) = require_form_security(&headers, &config, &customer, &form.csrf_token) {
        return request_security_error(error);
    }
    let Some(token) = customer_access_token(&headers) else {
        return deny_json(StatusCode::UNAUTHORIZED, "missing_customer_session");
    };
    let Some(supabase) = config.supabase_auth() else {
        return dependency_error(
            "supabase",
            "customer_login_not_configured",
            "SUPABASE_URL and SUPABASE_PUBLISHABLE_KEY are required",
        );
    };
    match supabase.enroll_totp(&token, "Fiducia authenticator").await {
        Ok(enrollment) => {
            let csrf = customer_csrf_token(&config, &customer);
            mfa_enroll_markup(&enrollment, &csrf).into_response()
        }
        Err(error) => {
            let detail = error.to_string();
            supabase_auth_error_response(
                error,
                mfa_result_markup("Couldn't start enrollment", &detail).into_response(),
                StatusCode::OK,
            )
        }
    }
}

/// `POST /app/security/mfa/activate` — confirm a freshly enrolled factor by
/// verifying its first code (challenge + verify).
pub(crate) async fn customer_mfa_activate(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Form(form): Form<MfaActivateForm>,
) -> Response {
    let customer = match config.authenticator.authenticate(&headers).await {
        Ok(customer) => customer,
        Err(response) => return response,
    };
    if let Err(error) = require_form_security(&headers, &config, &customer, &form.csrf_token) {
        return request_security_error(error);
    }
    let Some(token) = customer_access_token(&headers) else {
        return deny_json(StatusCode::UNAUTHORIZED, "missing_customer_session");
    };
    let Some(supabase) = config.supabase_auth() else {
        return dependency_error(
            "supabase",
            "customer_login_not_configured",
            "SUPABASE_URL and SUPABASE_PUBLISHABLE_KEY are required",
        );
    };
    let challenge = match supabase.challenge(&token, &form.factor_id).await {
        Ok(challenge) => challenge,
        Err(error) => return dependency_error("supabase", "mfa_challenge_failed", error),
    };
    match supabase.verify_factor(&token, &challenge, &form.code).await {
        Ok(session) => {
            // Verification upgrades the Supabase token to aal2. Rotate the app
            // cookie immediately so the user can manage the newly enrolled
            // factor without being trapped behind the MFA-aware app gate.
            let mut response = mfa_result_markup(
                "Authenticator enabled",
                "Your authenticator app is now required at sign-in. Keep your recovery method up to date.",
            )
            .into_response();
            append_set_cookie(
                &mut response,
                &make_customer_session_cookie(&session.access_token),
            );
            response
        }
        Err(error) => supabase_auth_error_response(
            error,
            mfa_result_markup(
                "That code didn't match",
                "Enrollment is not active. Return to security and start again with the current code.",
            )
            .into_response(),
            StatusCode::OK,
        ),
    }
}

/// `POST /app/security/mfa/disable` — unenroll a factor.
pub(crate) async fn customer_mfa_disable(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Form(form): Form<MfaDisableForm>,
) -> Response {
    let customer = match config.authenticator.authenticate(&headers).await {
        Ok(customer) => customer,
        Err(response) => return response,
    };
    // Supabase itself also enforces this today, but keep the assurance check at
    // our boundary: factor removal is an account-takeover primitive and must
    // never depend on a downstream policy remaining configured. The router-wide
    // gate additionally prevents an aal1 session for an enrolled account from
    // reaching this handler at all.
    if !customer.is_aal2() {
        return deny_json(StatusCode::UNAUTHORIZED, "mfa_step_up_required");
    }
    if let Err(error) = require_form_security(&headers, &config, &customer, &form.csrf_token) {
        return request_security_error(error);
    }
    let Some(token) = customer_access_token(&headers) else {
        return deny_json(StatusCode::UNAUTHORIZED, "missing_customer_session");
    };
    let Some(supabase) = config.supabase_auth() else {
        return dependency_error(
            "supabase",
            "customer_login_not_configured",
            "SUPABASE_URL and SUPABASE_PUBLISHABLE_KEY are required",
        );
    };
    // Re-challenge the exact factor immediately before deletion. This makes
    // removal a fresh possession proof, not merely a reuse of an earlier aal2
    // session. Use the returned stepped-up access token for the destructive
    // call so Supabase receives the same assurance we just established.
    let challenge = match supabase.challenge(&token, &form.factor_id).await {
        Ok(challenge) => challenge,
        Err(error) => {
            return supabase_auth_error_response(
                error,
                mfa_result_markup(
                    "Couldn't confirm your authenticator",
                    "Enter a current code from the authenticator you want to remove.",
                )
                .into_response(),
                StatusCode::UNAUTHORIZED,
            );
        }
    };
    let stepped_up = match supabase.verify_factor(&token, &challenge, &form.code).await {
        Ok(session) => session,
        Err(error) => {
            return supabase_auth_error_response(
                error,
                mfa_result_markup(
                    "That code didn't match",
                    "The authenticator was not removed. Enter its current 6-digit code and try again.",
                )
                .into_response(),
                StatusCode::UNAUTHORIZED,
            );
        }
    };
    match supabase
        .unenroll(&stepped_up.access_token, &form.factor_id)
        .await
    {
        Ok(()) => mfa_result_markup(
            "Authenticator removed",
            "That authenticator will no longer be requested at sign-in.",
        )
        .into_response(),
        Err(error) => dependency_error("supabase", "mfa_unenroll_failed", error),
    }
}

pub(crate) fn deny_json(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({ "ok": false, "error": code }))).into_response()
}

pub(crate) fn mfa_settings_markup(
    factors: &[supabase_auth::Factor],
    csrf_token: &str,
    message: Option<&str>,
) -> Markup {
    auth_page_shell(
        "Authenticator · Fiducia Customer",
        html! {
            p class="eyebrow" { "Security" }
            h1 { "Authenticator app (2FA)" }
            p class="muted" { "Add a TOTP authenticator (Authy, Google Authenticator, 1Password…) for a second factor at sign-in." }
            @if let Some(message) = message {
                p class="auth-message" role="alert" { (message) }
            }
            @if factors.is_empty() {
                p class="muted" { "No authenticator is enrolled yet." }
            } @else {
                ul class="factor-list" {
                    @for factor in factors {
                        li {
                            span { (factor.friendly_name.clone().unwrap_or_else(|| "Authenticator".to_string())) }
                            " — "
                            span { (factor.status.clone().unwrap_or_else(|| "unknown".to_string())) }
                            form method="post" action="/app/security/mfa/disable" hx-post="/app/security/mfa/disable" hx-target="body" hx-swap="outerHTML" {
                                input type="hidden" name="csrf_token" value=(csrf_token);
                                input type="hidden" name="factor_id" value=(factor.id);
                                label {
                                    "Current code from this authenticator"
                                    input name="code" type="text" inputmode="numeric" autocomplete="one-time-code"
                                        pattern="[0-9]*" minlength="6" maxlength="8" required;
                                }
                                button type="submit" class="link-button" { "Remove" }
                            }
                        }
                    }
                }
            }
            form method="post" action="/app/security/mfa/enroll" hx-post="/app/security/mfa/enroll" hx-target="body" hx-swap="outerHTML" {
                input type="hidden" name="csrf_token" value=(csrf_token);
                button type="submit" { "Add authenticator" }
            }
            a href="/app/security" { "Back to security" }
        },
    )
}

pub(crate) fn mfa_enroll_markup(
    enrollment: &supabase_auth::TotpEnrollment,
    csrf_token: &str,
) -> Markup {
    auth_page_shell(
        "Set up authenticator · Fiducia Customer",
        html! {
            p class="eyebrow" { "Security" }
            h1 { "Scan this with your authenticator" }
            p class="muted" { "Scan the QR code with Authy, Google Authenticator, or any TOTP app, then enter the 6-digit code it shows to finish." }
            div class="totp-qr" {
                (render_qr(&enrollment.qr_code))
            }
            p class="muted" { "Can't scan? Enter this key manually:" }
            pre class="totp-secret" { (enrollment.secret) }
            details class="totp-uri" {
                summary { "Prefer a setup link?" }
                p class="muted" { "Paste this otpauth URI into your authenticator app:" }
                pre class="totp-secret" { (enrollment.uri) }
            }
            form method="post" action="/app/security/mfa/activate" hx-post="/app/security/mfa/activate" hx-target="body" hx-swap="outerHTML" {
                input type="hidden" name="csrf_token" value=(csrf_token);
                input type="hidden" name="factor_id" value=(enrollment.factor_id);
                label for="activate-code" { "Code from your app" }
                input id="activate-code" name="code" type="text" inputmode="numeric" autocomplete="one-time-code"
                    pattern="[0-9]*" minlength="6" maxlength="8" required;
                button type="submit" { "Turn on 2FA" }
            }
            a href="/app/security/mfa" { "Cancel" }
        },
    )
}

/// Render Supabase's QR payload. A `data:`/`http` URL becomes an `img`; anything
/// else is treated as inline SVG markup from the trusted first-party IdP.
pub(crate) fn render_qr(qr_code: &str) -> Markup {
    let trimmed = qr_code.trim();
    if trimmed.starts_with("data:") || trimmed.starts_with("http") {
        html! { img src=(trimmed) alt="Authenticator QR code" width="200" height="200"; }
    } else {
        html! { (PreEscaped(trimmed.to_string())) }
    }
}

pub(crate) fn mfa_result_markup(title: &str, message: &str) -> Markup {
    auth_page_shell(
        "Security · Fiducia Customer",
        html! {
            p class="eyebrow" { "Security" }
            h1 { (title) }
            p class="muted" { (message) }
            a href="/app/security/mfa" { "Manage authenticator" }
            " · "
            a href="/app/security" { "Back to security" }
        },
    )
}
