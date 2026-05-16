use axum::body::Body;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "static/"]
struct Assets;

pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    // Default route → index.html (SPA shell).
    let asset_name = if path.is_empty() {
        "index.html"
    } else {
        path
    };

    match Assets::get(asset_name) {
        Some(content) => {
            let mime = mime_guess::from_path(asset_name).first_or_octet_stream();
            Response::builder()
                .header(header::CONTENT_TYPE, mime.as_ref())
                .header(header::CACHE_CONTROL, "public, max-age=300")
                .body(Body::from(content.data))
                .unwrap()
        }
        None => {
            // SPA fallback: serve index.html so deep links work.
            if let Some(idx) = Assets::get("index.html") {
                Response::builder()
                    .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                    .body(Body::from(idx.data))
                    .unwrap()
            } else {
                (StatusCode::NOT_FOUND, "not found").into_response()
            }
        }
    }
}
