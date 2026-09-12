use axum::{
    http::{header, HeaderValue},
    response::{Html, IntoResponse, Response},
};

pub async fn api_docs_html() -> Html<&'static str> {
    Html(include_str!("../generated/api-docs.html"))
}

pub async fn api_docs_json() -> Response {
    let mut response = include_str!("../generated/api-docs.json").into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    response
}
