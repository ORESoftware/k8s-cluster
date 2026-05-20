pub async fn html() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../generated/api-docs.html"))
}

pub async fn json() -> impl axum::response::IntoResponse {
    (
        [("content-type", "application/json; charset=utf-8")],
        include_str!("../generated/api-docs.json"),
    )
}
