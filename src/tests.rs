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

fn test_config(base: &std::path::Path, db_url: &str) -> Config {
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
cert_dir = "{base}"

[monitor]
interval_secs = 15
retention_days = 30
visitor_skip_paths = ["/metrics"]

[runtime]
driver = "proc"
unit_dir = "{base}/units"
bin_dir = "{base}/bin"
state_dir = "{base}/state"
env_dir = "{base}/etc"
artifact_dir = "{base}/artifacts"

[grpc]
enabled = false
host = "127.0.0.1"
port = 0

[agent]
join_token = "test-join"
"#,
        base = base.display()
    );
    Config::from_toml(&toml).expect("test config")
}

async fn test_state(dir: &std::path::Path) -> AppState {
    let url = format!("sqlite://{}/test.db?mode=rwc", dir.display());
    let cfg = Arc::new(test_config(dir, &url));
    let pool = db::connect(&cfg.database.url).await.expect("db");
    db::migrate(&pool).await.expect("migrate");
    AppState::for_test(cfg, pool)
}

async fn test_router(dir: &std::path::Path) -> axum::Router {
    app::build_router(test_state(dir).await)
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

async fn create_app(router: &axum::Router, name: &str, binary: &str, port: u16) -> String {
    let payload = json!({"name": name, "binary_path": binary, "port": port});
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
    body_json(resp).await["application"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn rollback_without_previous_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    let id = create_app(&router, "rapp", "/bin/true", 9200).await;
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/apps/{id}/rollback"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn deploy_stores_and_serves_artifact() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    let id = create_app(&router, "dapp", "/usr/bin/true", 9201).await;

    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/apps/{id}/deploy"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let hash = body["artifact_hash"].as_str().unwrap().to_string();
    assert!(hash.starts_with("sha256:"));

    let resp = router
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/artifacts/{hash}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    assert!(!bytes.is_empty());
}

#[tokio::test]
async fn agent_register_and_heartbeat() {
    use crate::grpc::pb::{HeartbeatRequest, RegisterRequest};

    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;

    let req = |token: &str| RegisterRequest {
        join_token: token.to_string(),
        name: "w1".into(),
        address: "10.0.0.9".into(),
        version: "0".into(),
    };

    assert!(crate::grpc::register(&state, req("wrong")).await.is_err());

    let reg = crate::grpc::register(&state, req("test-join"))
        .await
        .unwrap();
    assert!(!reg.server_id.is_empty());
    assert!(!reg.agent_token.is_empty());

    let hb = crate::grpc::heartbeat(
        &state,
        HeartbeatRequest {
            agent_token: reg.agent_token.clone(),
        },
    )
    .await
    .unwrap();
    assert!(hb.ok);
    assert_eq!(hb.server_id, reg.server_id);

    let bad = crate::grpc::heartbeat(
        &state,
        HeartbeatRequest {
            agent_token: "bad".into(),
        },
    )
    .await;
    assert!(bad.is_err());
}

#[tokio::test]
async fn agent_poll_and_report() {
    use crate::grpc::pb::{PollRequest, RegisterRequest, ReportRequest};

    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let router = app::build_router(state.clone());

    let reg = crate::grpc::register(
        &state,
        RegisterRequest {
            join_token: "test-join".into(),
            name: "pollw".into(),
            address: "10.0.0.5".into(),
            version: "0".into(),
        },
    )
    .await
    .unwrap();

    let payload = json!({
        "name": "papp", "binary_path": "/usr/bin/true", "port": 9400,
        "server_id": reg.server_id
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
    let id = body_json(resp).await["application"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Deploy queues the artifact for the agent.
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/apps/{id}/deploy"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let hash = body_json(resp).await["artifact_hash"]
        .as_str()
        .unwrap()
        .to_string();

    let poll = crate::grpc::poll(
        &state,
        PollRequest {
            agent_token: reg.agent_token.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(poll.apps.len(), 1);
    assert_eq!(poll.apps[0].name, "papp");
    assert_eq!(poll.apps[0].artifact_hash, hash);

    let report = crate::grpc::report(
        &state,
        ReportRequest {
            agent_token: reg.agent_token,
            app_name: "papp".into(),
            status: "running".into(),
            artifact_hash: hash,
            message: String::new(),
        },
    )
    .await
    .unwrap();
    assert!(report.ok);

    let status: String = sqlx::query_scalar("SELECT status FROM applications WHERE id = ?")
        .bind(&id)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(status, "running");
}

#[tokio::test]
async fn agent_push_metrics_rolls_up() {
    use crate::grpc::pb::{AppMetrics, MetricsRequest, RegionVisits, RegisterRequest};

    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let router = app::build_router(state.clone());

    let reg = crate::grpc::register(
        &state,
        RegisterRequest {
            join_token: "test-join".into(),
            name: "mw".into(),
            address: "10.0.0.7".into(),
            version: "0".into(),
        },
    )
    .await
    .unwrap();

    let payload = json!({
        "name": "mapp", "binary_path": "/usr/bin/true", "port": 9500,
        "server_id": reg.server_id
    });
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
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = crate::grpc::push_metrics(
        &state,
        MetricsRequest {
            agent_token: reg.agent_token,
            apps: vec![AppMetrics {
                app_name: "mapp".into(),
                cpu_pct: 12.5,
                mem_bytes: 1024,
                visits: vec![RegionVisits {
                    region: "ID".into(),
                    visits: 3,
                    uniques: 2,
                }],
            }],
        },
    )
    .await
    .unwrap();
    assert!(resp.ok);

    let cpu: f64 = sqlx::query_scalar(
        "SELECT cpu_pct FROM app_metrics WHERE application_id = (SELECT id FROM applications WHERE name='mapp')",
    )
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert_eq!(cpu, 12.5);

    let (visits, uniques): (i64, i64) = sqlx::query_as(
        "SELECT visits, uniques FROM visit_metrics WHERE application_id = (SELECT id FROM applications WHERE name='mapp')",
    )
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert_eq!(visits, 3);
    assert_eq!(uniques, 2);
}

#[tokio::test]
async fn deployment_history_endpoint() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    let id = create_app(&router, "happ", "/usr/bin/true", 9600).await;

    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/apps/{id}/deploy"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/apps/{id}/deployments"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let deps = body["deployments"].as_array().unwrap();
    assert_eq!(deps.len(), 1);
    let dep_id = deps[0]["id"].as_str().unwrap().to_string();
    assert!(deps[0]["artifact_hash"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));

    let resp = router
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/deployments/{dep_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert!(body["deployment"]["log"].is_string());
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
