//! Embedded frontend. The `static/` directory is compiled into the binary, so
//! turaes ships as a single file with no external assets.

use axum::body::Body;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "static"]
struct Assets;

fn mime_for(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    }
}

fn asset_response(path: &str) -> Option<Response> {
    let file = Assets::get(path)?;
    let mime = mime_for(path);
    Some(
        (
            [(header::CONTENT_TYPE, mime)],
            Body::from(file.data.into_owned()),
        )
            .into_response(),
    )
}

/// Fallback handler: serve static assets, SPA-fallback unknown non-asset paths
/// to `index.html`, and return 404 for missing files.
pub async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    if let Some(resp) = asset_response(path) {
        return resp;
    }
    if !path.contains('.') {
        if let Some(resp) = asset_response("index.html") {
            return resp;
        }
    }
    StatusCode::NOT_FOUND.into_response()
}
