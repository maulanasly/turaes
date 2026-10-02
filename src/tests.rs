//! In-crate integration tests for the HTTP surface.
//!
//! Kept inside the binary crate so they can exercise `crate::app` without a
//! separate library target. Each test gets its own temporary database and
//! builds state directly (no process env), so tests run in parallel safely.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;

use crate::app;
use crate::state::AppState;
use turaes_core::{db, Config};

fn test_config(db_url: &str) -> Config {
    let toml = format!(
        r#"
[server]
host = "127.0.0.1"
port = 0
base_domain = "localhost"
public_url = "http://localhost"

[database]
url = "{db_url}"

[auth]
jwt_secret = "test-secret-that-is-long-enough-123456"
github_client_id = ""
github_client_secret = ""
allowed_github_ids = []
app_origin = "http://localhost"
session_ttl_days = 30
allow_insecure_cookies = false

[proxy]
enabled = false
http_port = 80
https_port = 443
cert_dir = "/tmp"

[monitor]
interval_secs = 15
retention_days = 30
visitor_skip_paths = ["/metrics"]

[runtime]
driver = "proc"
unit_dir = "/tmp"
bin_dir = "/tmp"
state_dir = "/tmp"
env_dir = "/tmp"
"#
    );
    Config::from_toml(&toml).expect("test config")
}

async fn test_router(dir: &std::path::Path) -> axum::Router {
    let url = format!("sqlite://{}/test.db?mode=rwc", dir.display());
    let cfg = Arc::new(test_config(&url));
    let pool = db::connect(&cfg.database.url).await.expect("db");
    db::migrate(&pool).await.expect("migrate");
    app::build_router(AppState::for_test(cfg, pool))
}

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn health_is_public() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    let resp = router
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn create_then_list_application() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;

    let payload = json!({
        "name": "beruang",
        "binary_path": "/srv/beruang/beruang-gateway",
        "port": 8000,
        "domain": "kalkulator.rayakala.ink"
    });
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/apps")
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let created = body_json(resp).await;
    assert_eq!(created["application"]["name"], "beruang");
    assert_eq!(created["application"]["status"], "stopped");

    let resp = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/apps")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let listed = body_json(resp).await;
    assert_eq!(listed["applications"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn local_server_is_registered() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    let resp = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/servers")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let servers = body["servers"].as_array().unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0]["id"], "local");
    assert_eq!(servers[0]["is_local"], true);
}

#[tokio::test]
async fn create_and_delete_server() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;

    let payload = json!({"name": "worker-1", "address": "10.0.0.12"});
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/servers")
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let created = body_json(resp).await;
    let id = created["server"]["id"].as_str().unwrap().to_string();
    assert_eq!(created["server"]["name"], "worker-1");

    // The local server cannot be removed.
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/servers/local")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // A remote server can.
    let resp = router
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/servers/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn invalid_name_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    let payload = json!({"name": "Bad Name", "binary_path": "/bin/true", "port": 1});
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/apps")
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}
