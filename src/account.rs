//! Account pages: B2C/B2B profile setup, two-factor enrollment, recent
//! sign-ins, and B2B API keys for ERP integrations.

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use maud::{html, Markup};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::auth::{self, AuthUser, Biz, MaybeUser};
use crate::db::{self, CustomerProfile, CustomerType};
use crate::{pages, SharedState};

fn hash_api_key(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Charge one attempt against the per-user second-factor code budget shared
/// with `/login/2fa`. `Err` carries the fully-formed 429 to return; `render`
/// is only invoked on rejection so the caller pays nothing on the happy path.
async fn throttle_code_attempt(
    state: &SharedState,
    user: &AuthUser,
    render: impl FnOnce() -> Markup,
) -> Result<(), Response> {
    if state
        .rate_limits
        .check(
            "mfa-verify",
            &user.id.to_string(),
            auth::MFA_VERIFY_MAX_ATTEMPTS,
            auth::MFA_VERIFY_WINDOW,
        )
        .await
    {
        return Ok(());
    }
    Err((axum::http::StatusCode::TOO_MANY_REQUESTS, render()).into_response())
}

fn new_api_key() -> String {
    format!(
        "athk_{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    )
}

// ---------------------------------------------------------------------------
// First-login setup: choose personal vs business.

/// GET /account/setup
///
/// Goes through `require_full` like every other account route: an AAL1
/// session belonging to a 2FA-enrolled user must finish step-up first. See
/// the note on [`setup_submit`] for why this matters here specifically.
pub async fn setup_page(State(state): State<SharedState>, user: MaybeUser, biz: Biz) -> Response {
    let (auth_user, profile) = match auth::require_full(&state, &user).await {
        Ok(pair) => pair,
        Err(response) => return response,
    };
    setup_form(&auth_user, biz, profile.as_ref(), None).into_response()
}

fn setup_form(
    user: &AuthUser,
    biz: Biz,
    profile: Option<&CustomerProfile>,
    notice: Option<Markup>,
) -> Markup {
    let is_b2b = profile.map(CustomerProfile::is_b2b).unwrap_or(biz.0);
    pages::layout_for(
        "Account setup | AthletO",
        Some(user),
        biz,
        html! {
            section .section .auth-section {
                div .auth-card {
                    p .eyebrow { "Almost there" }
                    h2 { "How will you order?" }
                    p .auth-lede { "Signed in as " strong { (user.email_str()) } "." }
                    @if let Some(notice) = notice { (notice) }
                    form method="post" action="/account/setup" {
                        (pages::csrf_field())
                        label .radio-row {
                            input type="radio" name="customer_type" value="b2c" checked[!is_b2b];
                            span { strong { "Personal" } " -- one-time or recurring orders for you or your squad" }
                        }
                        label .radio-row {
                            input type="radio" name="customer_type" value="b2b" checked[is_b2b];
                            span { strong { "Business" } " -- wholesale cases, purchase orders, and ERP/EDI integration" }
                        }
                        label {
                            "Company name " span .muted-inline { "(required for business accounts)" }
                            input type="text" name="company_name" maxlength="120"
                                value=(profile.and_then(|p| p.company_name.as_deref()).unwrap_or(""))
                                placeholder="Wobble Distribution Inc.";
                        }
                        button .primary type="submit" { "Save and continue" }
                    }
                    p .auth-alt {
                        "Business accounts require approval and two-factor authentication before placing orders."
                    }
                }
            }
        },
    )
}

#[derive(Debug, Deserialize)]
pub struct SetupRequest {
    customer_type: String,
    #[serde(default)]
    company_name: String,
}

/// POST /account/setup
///
/// Gated by `require_full`, not just a session check. This handler calls
/// `db::upsert_profile`, which clears `b2b_approved_at` on a downgrade to
/// b2c. Without the AAL2 gate, someone holding a magic link for a
/// 2FA-enrolled account could reach a live AAL1 session and destroy a vetted
/// B2B account's approval before ever completing step-up — an ops re-approval
/// round trip triggered pre-2FA.
pub async fn setup_submit(
    State(state): State<SharedState>,
    user: MaybeUser,
    biz: Biz,
    Form(request): Form<SetupRequest>,
) -> Response {
    let (auth_user, _) = match auth::require_full(&state, &user).await {
        Ok(pair) => pair,
        Err(response) => return response,
    };
    let auth_user = &auth_user;
    let Some(pool) = &state.pool else {
        return pages::layout_for(
            "Account setup | AthletO",
            Some(auth_user),
            biz,
            html! { section .section { (pages::not_configured_notice("The account database")) } },
        )
        .into_response();
    };

    let customer_type = match request.customer_type.as_str() {
        "b2b" => CustomerType::B2b,
        _ => CustomerType::B2c,
    };
    let company = request.company_name.trim();
    if customer_type == CustomerType::B2b && company.is_empty() {
        return setup_form(
            auth_user,
            biz,
            None,
            Some(html! { div .notice .error { "Business accounts need a company name." } }),
        )
        .into_response();
    }

    let company_opt = (!company.is_empty()).then_some(company);
    if let Err(err) = db::upsert_profile(pool, auth_user.id, customer_type, company_opt).await {
        tracing::error!(error = %err, "profile upsert failed");
        return setup_form(
            auth_user,
            biz,
            None,
            Some(html! { div .notice .error { "Could not save your profile. Try again." } }),
        )
        .into_response();
    }

    match customer_type {
        CustomerType::B2b => Redirect::to("/account?approval=pending").into_response(),
        CustomerType::B2c => Redirect::to("/").into_response(),
    }
}

// ---------------------------------------------------------------------------
// Account overview + security.

#[derive(Debug, Default, Deserialize)]
pub struct AccountParams {
    approval: Option<String>,
    required2fa: Option<String>,
    enrolled: Option<String>,
    error: Option<String>,
}

/// GET /account
pub async fn account_page(
    State(state): State<SharedState>,
    user: MaybeUser,
    biz: Biz,
    Query(params): Query<AccountParams>,
) -> Response {
    let (auth_user, profile) = match auth::require_full(&state, &user).await {
        Ok(pair) => pair,
        Err(redirect) => return redirect,
    };
    if profile.is_none() && state.pool.is_some() {
        return Redirect::to("/account/setup").into_response();
    }

    let recent = match &state.pool {
        Some(pool) => db::recent_login_events(pool, auth_user.id, 5)
            .await
            .unwrap_or_default(),
        None => Vec::new(),
    };
    let api_keys = match (&state.pool, profile.as_ref()) {
        (Some(pool), Some(profile)) if profile.is_b2b_approved() => {
            db::list_api_keys(pool, auth_user.id)
                .await
                .unwrap_or_default()
        }
        _ => Vec::new(),
    };

    account_markup(
        &state,
        &auth_user,
        biz,
        profile.as_ref(),
        &recent,
        &api_keys,
        &params,
        None,
    )
    .into_response()
}

#[allow(clippy::too_many_arguments)]
fn account_markup(
    state: &SharedState,
    user: &AuthUser,
    biz: Biz,
    profile: Option<&CustomerProfile>,
    recent: &[db::LoginEvent],
    api_keys: &[db::ApiKeyRow],
    params: &AccountParams,
    extra: Option<Markup>,
) -> Markup {
    let is_b2b = profile
        .map(CustomerProfile::is_b2b_approved)
        .unwrap_or(false);
    let b2b_pending = profile
        .map(|profile| profile.is_b2b() && !profile.is_b2b_approved())
        .unwrap_or(false);
    let verified_count = user.verified_factors().count();
    pages::layout_for(
        "Account | AthletO",
        Some(user),
        biz,
        html! {
            section .section {
                h2 { "Your account" }
                @if params.approval.is_some() || b2b_pending {
                    div .notice {
                        strong { "Business account approval pending. " }
                        "Net-30 terms, ERP API keys, and business ordering activate after operations approves your company."
                    }
                }
                @if params.required2fa.is_some() {
                    div .notice .error {
                        strong { "Business accounts require two-factor authentication. " }
                        "Set up an authenticator app below before placing orders."
                    }
                }
                @if params.enrolled.is_some() {
                    div .notice .success { "Two-factor authentication is on. Codes will be required at sign-in." }
                }
                @if let Some(error) = params.error.as_deref() {
                    div .notice .error { (error) }
                }
                @if let Some(extra) = &extra { (extra) }

                div .account-grid {
                    div .account-card {
                        h3 { "Profile" }
                        p {
                            strong { (user.email_str()) }
                            @if let Some(profile) = profile {
                                br;
                                span .muted-inline { (profile.customer_type.label()) " account" }
                                @if let Some(company) = profile.company_name.as_deref() {
                                    " -- " (company)
                                }
                            }
                        }
                        p .auth-alt { a href="/account/setup" { "Change account type or company" } }
                    }

                    div .account-card #security {
                        h3 { "Two-factor authentication" }
                        @if user.factors.is_empty() {
                            p .auth-alt { "No second factor enrolled yet." }
                        } @else {
                            ul .factor-list {
                                @for factor in &user.factors {
                                    li {
                                        strong {
                                            @if factor.factor_type == "totp" { "Authenticator app" }
                                            @else if factor.factor_type == "phone" { "SMS" }
                                            @else { (factor.factor_type) }
                                        }
                                        " -- " (factor.friendly_name.as_deref().unwrap_or("unnamed"))
                                        " -- "
                                        @if factor.is_verified() { span .ok-inline { "verified" } }
                                        @else { span .muted-inline { "pending" } }
                                        form .inline-form method="post"
                                            action=(format!("/account/2fa/{}/unenroll", factor.id)) {
                                            (pages::csrf_field())
                                            button .linklike .danger-link type="submit" { "remove" }
                                        }
                                    }
                                }
                            }
                        }
                        @if is_b2b && verified_count == 1 {
                            p .auth-alt { "Business accounts must keep at least one verified factor." }
                        }
                        form method="post" action="/account/2fa/totp" {
                            (pages::csrf_field())
                            button .primary type="submit" { "Set up authenticator app (TOTP)" }
                        }
                        @if state.config.sms_mfa_enabled {
                            form method="post" action="/account/2fa/phone" {
                                (pages::csrf_field())
                                label {
                                    "Phone number for SMS codes"
                                    input type="tel" name="phone" placeholder="+15551234567" required;
                                }
                                button type="submit" { "Set up SMS codes" }
                            }
                        } @else {
                            p .auth-alt {
                                "SMS codes are ready in the app but need the Supabase phone-MFA "
                                "add-on and a Twilio account before they can be switched on."
                            }
                        }
                    }

                    div .account-card {
                        h3 { "Recent sign-ins" }
                        @if recent.is_empty() {
                            p .auth-alt { "No sign-ins recorded yet." }
                        } @else {
                            ul .factor-list {
                                @for event in recent {
                                    li {
                                        (event.email) " -- "
                                        span .muted-inline { (event.created_at.format("%b %-d, %H:%M UTC")) }
                                        @if event.aal == "aal2" { " -- " span .ok-inline { "2FA" } }
                                    }
                                }
                            }
                        }
                    }

                    @if is_b2b {
                        div .account-card #api-keys {
                            h3 { "ERP API keys" }
                            p .auth-alt {
                                "Server-to-server keys for your ERP or EDI provider. "
                                "Calls go to " code { "/api/v1" } " with "
                                code { "Authorization: Bearer <key>" } "."
                            }
                            @if api_keys.is_empty() {
                                p .auth-alt { "No keys yet." }
                            } @else {
                                ul .factor-list {
                                    @for key in api_keys {
                                        li {
                                            strong { (key.name) }
                                            " -- " code { (key.prefix) "..." }
                                            " -- created " span .muted-inline { (key.created_at.format("%b %-d")) }
                                            @if let Some(last) = key.last_used_at {
                                                " -- last used " span .muted-inline { (last.format("%b %-d, %H:%M UTC")) }
                                            }
                                            @if key.revoked_at.is_some() {
                                                " -- " span .muted-inline { "revoked" }
                                            } @else {
                                                form .inline-form method="post"
                                                    action=(format!("/account/api-keys/{}/revoke", key.id)) {
                                                    (pages::csrf_field())
                                                    button .linklike .danger-link type="submit" { "revoke" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            form method="post" action="/account/api-keys" {
                                (pages::csrf_field())
                                label {
                                    "Key name"
                                    input type="text" name="name" required maxlength="60"
                                        placeholder="SPS Commerce production";
                                }
                                button .primary type="submit" { "Create API key" }
                            }
                        }
                    }
                }
            }
        },
    )
}

// ---------------------------------------------------------------------------
// TOTP enrollment.

/// POST /account/2fa/totp -- enroll a TOTP factor and show the QR code.
pub async fn totp_enroll(State(state): State<SharedState>, user: MaybeUser, biz: Biz) -> Response {
    let Some(auth_user) = user.as_ref() else {
        return Redirect::to("/login").into_response();
    };
    // A session that already has a verified factor but is still AAL1 must step
    // up through /login/2fa first; otherwise a pre-2FA session (e.g. a
    // forwarded magic link) could enroll a *new* attacker-controlled factor and
    // verify its way to AAL2, bypassing the existing factor entirely.
    if auth_user.needs_aal2() {
        return Redirect::to("/login/2fa").into_response();
    }
    let body = serde_json::json!({
        "factor_type": "totp",
        "friendly_name": format!("authenticator-{}", chrono::Utc::now().format("%Y%m%d%H%M%S")),
    });
    match auth::enroll_factor(&state, &auth_user.access_token, body).await {
        Ok(enrolled) => {
            let factor_id = enrolled
                .get("id")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            let qr = enrolled
                .pointer("/totp/qr_code")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            let secret = enrolled
                .pointer("/totp/secret")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            totp_verify_page(auth_user, biz, &factor_id, &qr, &secret, None).into_response()
        }
        Err(message) => {
            Redirect::to(&format!("/account?error={}", urlencoding_light(&message))).into_response()
        }
    }
}

fn urlencoding_light(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '.' | '_' => c.to_string(),
            ' ' => "+".to_string(),
            _ => format!("%{:02X}", c as u32),
        })
        .collect()
}

fn totp_verify_page(
    user: &AuthUser,
    biz: Biz,
    factor_id: &str,
    qr: &str,
    secret: &str,
    notice: Option<Markup>,
) -> Markup {
    pages::layout_for(
        "Set up authenticator | AthletO",
        Some(user),
        biz,
        html! {
            section .section .auth-section {
                div .auth-card {
                    p .eyebrow { "Two-factor setup" }
                    h2 { "Scan with your authenticator" }
                    p .auth-lede {
                        "Scan the QR code with Authy, Google Authenticator, 1Password, or any "
                        "TOTP app, then enter the 6-digit code it shows."
                    }
                    @if let Some(notice) = notice { (notice) }
                    div .qr-box {
                        // Only ever render the QR as a data: image URI. The old
                        // fallback emitted `qr` as raw HTML, and `qr` is
                        // round-tripped through a hidden form field on verify --
                        // so a crafted `qr=<img onerror=...>` would inject
                        // markup. When the code is not a data: URI, fall back to
                        // the manual secret shown below instead of raw output.
                        @if qr.starts_with("data:") {
                            img src=(qr) alt="TOTP QR code" width="220" height="220";
                        } @else {
                            p .auth-alt { "Enter the setup key below into your authenticator app." }
                        }
                    }
                    p .auth-alt { "Can't scan? Enter this secret manually: " code { (secret) } }
                    form method="post" action="/account/2fa/totp/verify" {
                        (pages::csrf_field())
                        input type="hidden" name="factor_id" value=(factor_id);
                        input type="hidden" name="qr" value=(qr);
                        input type="hidden" name="secret" value=(secret);
                        label {
                            "6-digit code"
                            input .code-input type="text" name="code" inputmode="numeric"
                                pattern="[0-9]*" minlength="6" maxlength="8" required
                                autocomplete="one-time-code" placeholder="123456";
                        }
                        button .primary type="submit" { "Turn on 2FA" }
                    }
                }
            }
        },
    )
}

#[derive(Debug, Deserialize)]
pub struct TotpVerifyRequest {
    factor_id: String,
    code: String,
    #[serde(default)]
    qr: String,
    #[serde(default)]
    secret: String,
}

/// POST /account/2fa/totp/verify -- confirm the first code; session becomes AAL2.
pub async fn totp_verify(
    State(state): State<SharedState>,
    user: MaybeUser,
    biz: Biz,
    jar: CookieJar,
    Form(request): Form<TotpVerifyRequest>,
) -> Response {
    let Some(auth_user) = user.as_ref() else {
        return Redirect::to("/login").into_response();
    };
    // See totp_enroll: block a mid-step-up session from verifying a freshly
    // enrolled factor to reach AAL2 around an existing one.
    if auth_user.needs_aal2() {
        return Redirect::to("/login/2fa").into_response();
    }
    // Enrollment verification mints an AAL2 session too, so it gets the same
    // attempt budget as /login/2fa rather than an unbounded one.
    if let Err(response) = throttle_code_attempt(&state, auth_user, || {
        totp_verify_page(
            auth_user,
            biz,
            &request.factor_id,
            &request.qr,
            &request.secret,
            Some(html! { div .notice .error {
                "Too many verification attempts. Wait a few minutes and try again."
            } }),
        )
    })
    .await
    {
        return response;
    }
    let challenge =
        match auth::create_challenge(&state, &auth_user.access_token, &request.factor_id).await {
            Ok(challenge) => challenge,
            Err(message) => {
                return totp_verify_page(
                    auth_user,
                    biz,
                    &request.factor_id,
                    &request.qr,
                    &request.secret,
                    Some(html! { div .notice .error { (message) } }),
                )
                .into_response()
            }
        };
    match auth::verify_challenge(
        &state,
        &auth_user.access_token,
        &request.factor_id,
        &challenge,
        request.code.trim(),
    )
    .await
    {
        Ok((access, refresh)) => {
            let jar = jar
                .add(crate::auth_session_cookie(access))
                .add(crate::refresh_session_cookie(refresh));
            (jar, Redirect::to("/account?enrolled=1")).into_response()
        }
        Err(message) => totp_verify_page(
            auth_user,
            biz,
            &request.factor_id,
            &request.qr,
            &request.secret,
            Some(html! { div .notice .error { "That code didn't verify: " (message) } }),
        )
        .into_response(),
    }
}

// ---------------------------------------------------------------------------
// SMS (phone) factor enrollment -- gated on the Supabase phone-MFA add-on.

#[derive(Debug, Deserialize)]
pub struct PhoneEnrollRequest {
    phone: String,
}

/// POST /account/2fa/phone
pub async fn phone_enroll(
    State(state): State<SharedState>,
    user: MaybeUser,
    Form(request): Form<PhoneEnrollRequest>,
) -> Response {
    let Some(auth_user) = user.as_ref() else {
        return Redirect::to("/login").into_response();
    };
    // See totp_enroll: a mid-step-up session must clear /login/2fa before it
    // can enroll another factor.
    if auth_user.needs_aal2() {
        return Redirect::to("/login/2fa").into_response();
    }
    if !state.config.sms_mfa_enabled {
        return Redirect::to("/account?error=SMS+codes+are+not+enabled+on+this+deployment")
            .into_response();
    }
    let body = serde_json::json!({
        "factor_type": "phone",
        "friendly_name": "sms",
        "phone": request.phone.trim(),
    });
    match auth::enroll_factor(&state, &auth_user.access_token, body).await {
        Ok(enrolled) => {
            let factor_id = enrolled
                .get("id")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            match auth::create_challenge(&state, &auth_user.access_token, &factor_id).await {
                Ok(challenge) => {
                    phone_verify_page(auth_user, &factor_id, &challenge, None).into_response()
                }
                Err(message) => {
                    Redirect::to(&format!("/account?error={}", urlencoding_light(&message)))
                        .into_response()
                }
            }
        }
        Err(message) => {
            Redirect::to(&format!("/account?error={}", urlencoding_light(&message))).into_response()
        }
    }
}

fn phone_verify_page(
    user: &AuthUser,
    factor_id: &str,
    challenge_id: &str,
    notice: Option<Markup>,
) -> Markup {
    pages::layout(
        "Verify your phone | AthletO",
        Some(user),
        html! {
            section .section .auth-section {
                div .auth-card {
                    h2 { "Enter the code we texted" }
                    @if let Some(notice) = notice { (notice) }
                    form method="post" action="/account/2fa/phone/verify" {
                        (pages::csrf_field())
                        input type="hidden" name="factor_id" value=(factor_id);
                        input type="hidden" name="challenge_id" value=(challenge_id);
                        label {
                            "SMS code"
                            input .code-input type="text" name="code" inputmode="numeric"
                                pattern="[0-9]*" minlength="6" maxlength="8" required
                                autocomplete="one-time-code" placeholder="123456";
                        }
                        button .primary type="submit" { "Verify phone" }
                    }
                }
            }
        },
    )
}

#[derive(Debug, Deserialize)]
pub struct PhoneVerifyRequest {
    factor_id: String,
    challenge_id: String,
    code: String,
}

/// POST /account/2fa/phone/verify
pub async fn phone_verify(
    State(state): State<SharedState>,
    user: MaybeUser,
    jar: CookieJar,
    Form(request): Form<PhoneVerifyRequest>,
) -> Response {
    let Some(auth_user) = user.as_ref() else {
        return Redirect::to("/login").into_response();
    };
    // See totp_enroll: block a mid-step-up session from verifying a freshly
    // enrolled factor to reach AAL2 around an existing one.
    if auth_user.needs_aal2() {
        return Redirect::to("/login/2fa").into_response();
    }
    // Same attempt budget as /login/2fa; see totp_verify.
    if let Err(response) = throttle_code_attempt(&state, auth_user, || {
        phone_verify_page(
            auth_user,
            &request.factor_id,
            &request.challenge_id,
            Some(html! { div .notice .error {
                "Too many verification attempts. Wait a few minutes and try again."
            } }),
        )
    })
    .await
    {
        return response;
    }
    match auth::verify_challenge(
        &state,
        &auth_user.access_token,
        &request.factor_id,
        &request.challenge_id,
        request.code.trim(),
    )
    .await
    {
        Ok((access, refresh)) => {
            let jar = jar
                .add(crate::auth_session_cookie(access))
                .add(crate::refresh_session_cookie(refresh));
            (jar, Redirect::to("/account?enrolled=1")).into_response()
        }
        Err(message) => phone_verify_page(
            auth_user,
            &request.factor_id,
            &request.challenge_id,
            Some(html! { div .notice .error { "That code didn't verify: " (message) } }),
        )
        .into_response(),
    }
}

// ---------------------------------------------------------------------------
// Factor removal.

/// POST /account/2fa/{factor_id}/unenroll
pub async fn factor_unenroll(
    State(state): State<SharedState>,
    user: MaybeUser,
    Path(factor_id): Path<String>,
) -> Response {
    let (auth_user, profile) = match auth::require_full(&state, &user).await {
        Ok(pair) => pair,
        Err(redirect) => return redirect,
    };

    // B2B accounts may not drop below one verified factor -- that would
    // reopen the door 2FA is required to close.
    let is_last_verified = auth_user
        .verified_factors()
        .find(|factor| factor.id != factor_id)
        .is_none()
        && auth_user
            .factors
            .iter()
            .any(|factor| factor.id == factor_id && factor.is_verified());
    if profile
        .as_ref()
        .map(CustomerProfile::is_b2b)
        .unwrap_or(false)
        && is_last_verified
    {
        return Redirect::to(
            "/account?error=Business+accounts+must+keep+at+least+one+verified+factor",
        )
        .into_response();
    }

    match auth::unenroll_factor(&state, &auth_user.access_token, &factor_id).await {
        Ok(()) => Redirect::to("/account").into_response(),
        Err(message) => {
            Redirect::to(&format!("/account?error={}", urlencoding_light(&message))).into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// B2B API keys.

#[derive(Debug, Deserialize)]
pub struct ApiKeyCreateRequest {
    name: String,
}

/// POST /account/api-keys -- B2B + AAL2 only; shows the key exactly once.
pub async fn api_key_create(
    State(state): State<SharedState>,
    user: MaybeUser,
    biz: Biz,
    Form(request): Form<ApiKeyCreateRequest>,
) -> Response {
    let (auth_user, profile) = match auth::require_full(&state, &user).await {
        Ok(pair) => pair,
        Err(redirect) => return redirect,
    };
    if let Err(redirect) = auth::require_b2b_ready(&auth_user, profile.as_ref()) {
        return redirect;
    }
    let Some(profile) = profile.filter(CustomerProfile::is_b2b_approved) else {
        return Redirect::to("/account").into_response();
    };
    let Some(pool) = &state.pool else {
        return Redirect::to("/account?error=Database+not+configured").into_response();
    };

    let key = new_api_key();
    let prefix: String = key.chars().take(13).collect();
    let name = request.name.trim();
    let name = if name.is_empty() { "unnamed" } else { name };
    if let Err(err) =
        db::insert_api_key(pool, auth_user.id, name, &hash_api_key(&key), &prefix).await
    {
        tracing::error!(error = %err, "api key insert failed");
        return Redirect::to("/account?error=Could+not+create+the+key").into_response();
    }

    let recent = db::recent_login_events(pool, auth_user.id, 5)
        .await
        .unwrap_or_default();
    let api_keys = db::list_api_keys(pool, auth_user.id)
        .await
        .unwrap_or_default();
    let reveal = html! {
        div .notice .success {
            strong { "API key created -- copy it now, it will not be shown again:" }
            div .key-reveal { code { (key) } }
        }
    };
    account_markup(
        &state,
        &auth_user,
        biz,
        Some(&profile),
        &recent,
        &api_keys,
        &AccountParams::default(),
        Some(reveal),
    )
    .into_response()
}

/// POST /account/api-keys/{key_id}/revoke
pub async fn api_key_revoke(
    State(state): State<SharedState>,
    user: MaybeUser,
    Path(key_id): Path<Uuid>,
) -> Response {
    let (auth_user, profile) = match auth::require_full(&state, &user).await {
        Ok(pair) => pair,
        Err(redirect) => return redirect,
    };
    if let Err(redirect) = auth::require_b2b_ready(&auth_user, profile.as_ref()) {
        return redirect;
    }
    if let Some(pool) = &state.pool {
        if let Err(err) = db::revoke_api_key(pool, auth_user.id, key_id).await {
            tracing::error!(error = %err, "api key revoke failed");
        }
    }
    Redirect::to("/account").into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_api_key_matches_the_shared_vector() {
        // Must equal api::hash_key's vector so a key minted here verifies there.
        assert_eq!(
            hash_api_key("athk_test_vector_001"),
            "66adca3c7ae7f126ff03b7cc7daba157a1b9705447faaabd4fc1c2995c0d308a"
        );
    }

    #[test]
    fn new_api_key_is_prefixed_unique_and_display_prefix_is_stable() {
        let a = new_api_key();
        let b = new_api_key();
        assert!(a.starts_with("athk_"));
        assert_ne!(a, b, "keys must be unique");
        // The stored display prefix is the first 13 chars ("athk_" + 8 hex).
        let prefix: String = a.chars().take(13).collect();
        assert_eq!(prefix.len(), 13);
        assert!(a.starts_with(&prefix));
    }

    #[test]
    fn urlencoding_light_escapes_spaces_and_reserved() {
        assert_eq!(urlencoding_light("a b"), "a+b");
        assert_eq!(urlencoding_light("PO#1&2"), "PO%231%262");
        assert_eq!(urlencoding_light("plain.Text_1-2"), "plain.Text_1-2");
    }

    use crate::auth::{AuthUser, Factor, MaybeUser};
    use crate::{AppState, Config, SharedState};
    use axum::extract::State;
    use axum::http::{header, StatusCode};
    use std::sync::Arc;

    fn state() -> SharedState {
        Arc::new(AppState::new(
            None,
            reqwest::Client::new(),
            Config::default(),
        ))
    }

    fn aal1_user_with_verified_totp() -> AuthUser {
        AuthUser {
            id: uuid::Uuid::nil(),
            email: Some("victim@club.example".into()),
            aal: "aal1".into(),
            factors: vec![Factor {
                id: "existing".into(),
                factor_type: "totp".into(),
                status: "verified".into(),
                friendly_name: None,
            }],
            access_token: "t".into(),
        }
    }

    fn location(response: &axum::response::Response) -> &str {
        response
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
    }

    // Regression test for the 2FA step-up bypass: a session that already has a
    // verified factor but is still AAL1 (post-magic-link, pre-2FA) must not be
    // able to enroll or verify a *new* factor. The guard returns before any
    // GoTrue call, so this is exercisable in degraded mode.
    #[tokio::test]
    async fn factor_enroll_and_verify_reject_a_pre_2fa_session() {
        let user = MaybeUser(Some(aal1_user_with_verified_totp()));

        let enroll = totp_enroll(State(state()), user.clone(), Biz(false)).await;
        assert_eq!(enroll.status(), StatusCode::SEE_OTHER);
        assert_eq!(location(&enroll), "/login/2fa");

        let verify = totp_verify(
            State(state()),
            user.clone(),
            Biz(false),
            axum_extra::extract::cookie::CookieJar::new(),
            axum::extract::Form(TotpVerifyRequest {
                factor_id: "attacker".into(),
                code: "000000".into(),
                qr: String::new(),
                secret: String::new(),
            }),
        )
        .await;
        assert_eq!(verify.status(), StatusCode::SEE_OTHER);
        assert_eq!(location(&verify), "/login/2fa");

        let phone = phone_enroll(
            State(state()),
            user,
            axum::extract::Form(PhoneEnrollRequest {
                phone: "+15550000000".into(),
            }),
        )
        .await;
        assert_eq!(phone.status(), StatusCode::SEE_OTHER);
        assert_eq!(location(&phone), "/login/2fa");
    }

    // A first-time enroller (no verified factor yet) must still be allowed
    // through the guard -- otherwise nobody could ever turn 2FA on.
    #[tokio::test]
    async fn first_time_enroll_is_not_blocked_by_the_step_up_guard() {
        let fresh = MaybeUser(Some(AuthUser {
            id: uuid::Uuid::nil(),
            email: Some("new@club.example".into()),
            aal: "aal1".into(),
            factors: vec![],
            access_token: "t".into(),
        }));
        // No Supabase configured, so enrollment fails downstream and redirects
        // to /account with an error -- crucially NOT to /login/2fa, proving the
        // step-up guard did not fire for a factorless session.
        let enroll = totp_enroll(State(state()), fresh, Biz(false)).await;
        assert_eq!(enroll.status(), StatusCode::SEE_OTHER);
        assert_ne!(location(&enroll), "/login/2fa");
    }
}
