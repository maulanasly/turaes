//! Internal static file server: the supervision target for `static` apps.
//!
//! Serves a synced public directory on loopback with SPA fallback
//! (`{path}` → `{path}/index.html` → `/index.html`) and an always-200
//! `/health`, so the standard health gate, monitor and proxy paths work
//! unchanged. Path traversal is rejected by canonicalizing every request
//! against the root.

use std::path::{Path, PathBuf};

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use turaes_core::{Error, Result};

/// Shared server state.
#[derive(Debug, Clone)]
pub struct StaticState {
    /// Canonicalized document root.
    pub root: PathBuf,
}

/// Guess a content type from the file extension.
fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "xml" => "application/xml",
        "txt" => "text/plain; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "webmanifest" => "application/manifest+json",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// Resolve a request path to a file under `root`, following the SPA fallback
/// chain. Returns `None` when nothing matches (or on traversal attempts).
async fn resolve(root: &Path, uri_path: &str) -> Option<PathBuf> {
    let relative = uri_path.trim_start_matches('/');
    // Never join an absolute-looking candidate: `Path::join` would discard
    // the root and defeat the traversal check below.
    let mut candidates = vec![relative.to_string(), "index.html".to_string()];
    if !relative.is_empty() {
        candidates.insert(1, format!("{relative}/index.html"));
    }
    for candidate in candidates {
        if candidate.starts_with('/') {
            continue;
        }
        let joined = root.join(&candidate);
        let Ok(canonical) = tokio::fs::canonicalize(&joined).await else {
            continue;
        };
        if !canonical.starts_with(root) {
            continue;
        }
        if tokio::fs::metadata(&canonical)
            .await
            .is_ok_and(|m| m.is_file())
        {
            return Some(canonical);
        }
    }
    None
}

/// `GET /health` — always 200 so gates and monitors pass.
async fn health() -> &'static str {
    "ok"
}

/// `GET /*path` — the file server.
async fn serve_file(
    State(state): State<StaticState>,
    axum::extract::Path(path): axum::extract::Path<String>,
) -> Response {
    let Some(file) = resolve(&state.root, &path).await else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    match tokio::fs::read(&file).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, content_type(&file))], bytes).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// Build the router for a document root (canonicalized).
pub fn router(root: PathBuf) -> Result<Router> {
    let canonical = std::fs::canonicalize(&root)
        .map_err(|e| Error::BadRequest(format!("cannot serve {}: {e}", root.display())))?;
    let state = StaticState { root: canonical };
    Ok(Router::new()
        .route("/health", get(health))
        .route("/", get(serve_root))
        .route("/{*path}", get(serve_file))
        .with_state(state))
}

/// `GET /` — the wildcard does not match the bare root; serve the index.
async fn serve_root(State(state): State<StaticState>) -> Response {
    serve_file(State(state), axum::extract::Path(String::new())).await
}

/// Serve `dir` on `127.0.0.1:port` until killed.
pub async fn run(dir: PathBuf, port: u16) -> Result<()> {
    let app = router(dir)?;
    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| Error::Internal(format!("failed to bind {addr}: {e}")))?;
    tracing::info!(%addr, "serving static directory");
    axum::serve(listener, app)
        .await
        .map_err(|e| Error::Internal(format!("static server failed: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn body_text(resp: Response) -> (StatusCode, String, Option<String>) {
        let status = resp.status();
        let ctype = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&bytes).into_owned(), ctype)
    }

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "<h1>hi</h1>").unwrap();
        std::fs::write(dir.path().join("app.js"), "console.log(1)").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join("index.html"), "sub").unwrap();
        dir
    }

    #[tokio::test]
    async fn serves_files_types_and_spa_fallback() {
        let dir = fixture();
        let app = router(dir.path().to_path_buf()).unwrap();

        let get = |uri: &str| {
            app.clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        };
        let (status, body, _) = body_text(get("/health").await.unwrap()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "ok");

        let (status, body, ctype) = body_text(get("/").await.unwrap()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "<h1>hi</h1>");
        assert_eq!(ctype.as_deref(), Some("text/html; charset=utf-8"));

        let (status, body, ctype) = body_text(get("/app.js").await.unwrap()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "console.log(1)");
        assert_eq!(ctype.as_deref(), Some("text/javascript; charset=utf-8"));

        // SPA fallback: unknown deep path serves the root index.
        let (status, body, _) = body_text(get("/docs/getting-started").await.unwrap()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "<h1>hi</h1>");

        // Subdirectory index still resolves directly.
        let (status, body, _) = body_text(get("/sub").await.unwrap()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "sub");
    }

    #[tokio::test]
    async fn rejects_traversal_and_missing_root_index() {
        // No index anywhere: traversal must 404, never leak host files and
        // never render a directory listing.
        let empty = tempfile::tempdir().unwrap();
        std::fs::write(empty.path().join("other.txt"), "x").unwrap();
        let app = router(empty.path().to_path_buf()).unwrap();
        for uri in ["/", "/..%2f..%2fetc%2fpasswd"] {
            let resp = app
                .clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::NOT_FOUND, "for {uri}");
        }
        // ...except the real file, which serves fine.
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/other.txt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
