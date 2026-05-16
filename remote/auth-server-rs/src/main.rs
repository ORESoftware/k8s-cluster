use std::{env, net::SocketAddr};

use axum::{
    extract::{Form, Query},
    http::{header, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct AuthQuery {
    #[serde(rename = "return")]
    return_to: Option<String>,
}

#[derive(Deserialize)]
struct PinForm {
    pin: String,
    return_to: Option<String>,
}

#[derive(Serialize)]
struct HealthResponse {
    ok: bool,
    service: &'static str,
}

fn required_env(name: &str) -> String {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => value,
        _ => panic!("{name} must be configured"),
    }
}

fn auth_pin() -> String {
    required_env("DD_AUTH_PIN")
}

fn cookie_name() -> String {
    env::var("DD_AUTH_COOKIE_NAME").unwrap_or_else(|_| "dd_auth".to_string())
}

fn cookie_value() -> String {
    required_env("DD_AUTH_COOKIE_VALUE")
}

fn validate_required_config() {
    let _ = auth_pin();
    let _ = cookie_value();
}

fn safe_return_to(value: Option<String>) -> String {
    let Some(value) = value else {
        return "/home".to_string();
    };
    if value.starts_with('/') && !value.starts_with("//") {
        value
    } else {
        "/home".to_string()
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn login_page(return_to: &str, error: Option<&str>) -> Html<String> {
    let escaped_return = escape_html(return_to);
    let error_html = error
        .map(|message| format!(r#"<p class="error">{}</p>"#, escape_html(message)))
        .unwrap_or_default();
    Html(format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>dd remote auth</title>
    <style>
      :root {{ color-scheme: dark; }}
      body {{
        margin: 0;
        min-height: 100vh;
        display: grid;
        place-items: center;
        background: #0b1117;
        color: #eef2f6;
        font-family: Inter, ui-sans-serif, system-ui, -apple-system, Segoe UI, sans-serif;
      }}
      main {{
        width: min(420px, calc(100vw - 32px));
        border: 1px solid rgba(148, 163, 184, 0.24);
        border-radius: 8px;
        background: #111923;
        padding: 22px;
      }}
      h1 {{ margin: 0 0 8px; font-size: 22px; }}
      p {{ margin: 0 0 16px; color: #a8b3c1; line-height: 1.5; }}
      label {{ display: grid; gap: 8px; margin-bottom: 14px; }}
      input {{
        width: 100%;
        border: 1px solid rgba(148, 163, 184, 0.35);
        border-radius: 6px;
        background: #0a1017;
        color: #eef2f6;
        padding: 10px 12px;
        font: inherit;
      }}
      button {{
        border: 0;
        border-radius: 6px;
        background: #5eead4;
        color: #051014;
        cursor: pointer;
        font-weight: 700;
        padding: 10px 14px;
      }}
      code {{
        display: inline-block;
        max-width: 100%;
        overflow-wrap: anywhere;
        color: #d7fbf4;
      }}
      .error {{ color: #fca5a5; }}
    </style>
  </head>
  <body>
    <main>
      <h1>Remote runtime auth</h1>
      <p>Enter the operator PIN to set the browser cookie and return to <code>{escaped_return}</code>.</p>
      {error_html}
      <form method="post" action="/auth">
        <input type="hidden" name="return_to" value="{escaped_return}" />
        <label>
          PIN
          <input name="pin" inputmode="numeric" autocomplete="one-time-code" autofocus />
        </label>
        <button type="submit">Continue</button>
      </form>
    </main>
  </body>
</html>"#
    ))
}

async fn auth_form(Query(query): Query<AuthQuery>) -> impl IntoResponse {
    let return_to = safe_return_to(query.return_to);
    login_page(&return_to, None)
}

async fn auth_submit(Form(form): Form<PinForm>) -> Response {
    let return_to = safe_return_to(form.return_to);
    if form.pin.trim() != auth_pin() {
        return (
            StatusCode::UNAUTHORIZED,
            login_page(&return_to, Some("Incorrect PIN")),
        )
            .into_response();
    }

    let cookie = format!(
        "{}={}; Path=/; Max-Age=604800; HttpOnly; SameSite=Lax; Secure",
        cookie_name(),
        cookie_value()
    );
    let mut response = Response::new(axum::body::Body::empty());
    *response.status_mut() = StatusCode::SEE_OTHER;
    response.headers_mut().insert(
        header::LOCATION,
        HeaderValue::from_str(&return_to).unwrap_or_else(|_| HeaderValue::from_static("/home")),
    );
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).expect("auth cookie header should be valid"),
    );
    response
}

async fn healthz() -> impl IntoResponse {
    Json(HealthResponse {
        ok: true,
        service: "dd-remote-auth",
    })
}

#[tokio::main]
async fn main() {
    validate_required_config();

    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port = env::var("PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(8083);

    let app = Router::new()
        .route("/auth", get(auth_form).post(auth_submit))
        .route("/auth/", get(auth_form).post(auth_submit))
        .route("/healthz", get(healthz));

    let address: SocketAddr = format!("{host}:{port}")
        .parse()
        .expect("failed to parse bind address");
    println!("dd-remote-auth listening on http://{address}");

    let listener = tokio::net::TcpListener::bind(address)
        .await
        .expect("failed to bind tcp listener");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("axum server crashed");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut sigterm) = signal(SignalKind::terminate()) {
            let _ = sigterm.recv().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
