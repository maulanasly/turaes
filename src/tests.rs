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
cert_dir = "{base}/certs"
acme_webroot = "{base}/acme"

[monitor]
interval_secs = 15
retention_days = 30
visitor_skip_paths = ["/metrics"]

[alerts]
webhook_url = ""
backup_stale_hours = 48

[backup]
dir = "{base}/backups"
retain = 3

[runtime]
driver = "proc"
unit_dir = "{base}/units"
bin_dir = "{base}/bin"
state_dir = "{base}/state"
env_dir = "{base}/etc"
artifact_dir = "{base}/artifacts"
slot_offset = 1000
drain_secs = 0

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
                .uri("/api/v1/orgs/default/apps")
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
                .uri("/api/v1/orgs/default/apps")
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
async fn server_capacity_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;

    let now = chrono::Utc::now();
    let older = (now - chrono::Duration::hours(2))
        .format("%Y-%m-%d %H:%M:00")
        .to_string();
    let recent = (now - chrono::Duration::hours(1))
        .format("%Y-%m-%d %H:%M:00")
        .to_string();
    // Same minute twice: the minute-bucket upsert keeps one row.
    crate::monitor::upsert_host_metrics(
        &state.pool,
        "local",
        12.5,
        1_073_741_824,
        4_294_967_296,
        &older,
    )
    .await
    .unwrap();
    crate::monitor::upsert_host_metrics(
        &state.pool,
        "local",
        40.0,
        2_147_483_648,
        4_294_967_296,
        &recent,
    )
    .await
    .unwrap();
    crate::monitor::upsert_host_metrics(&state.pool, "local", 20.0, 1000, 4_294_967_296, &recent)
        .await
        .unwrap();

    let router = app::build_router(state.clone());
    // Fleet list carries the latest sample.
    let resp = router
        .clone()
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
    let cap = &body["servers"][0]["capacity"];
    assert_eq!(cap["cpu_pct"].as_f64().unwrap(), 20.0);
    assert_eq!(cap["mem_bytes"].as_i64().unwrap(), 1000);

    // History returns both distinct minutes, oldest first.
    let resp = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/servers/local/stats?hours=720")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let metrics = body["metrics"].as_array().unwrap();
    assert_eq!(metrics.len(), 2);
    assert_eq!(metrics[0]["cpu_pct"].as_f64().unwrap(), 12.5);
    assert_eq!(metrics[1]["cpu_pct"].as_f64().unwrap(), 20.0);
}

async fn create_app(router: &axum::Router, name: &str, binary: &str, port: u16) -> String {
    let payload = json!({"name": name, "binary_path": binary, "port": port});
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/apps")
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

/// Write a tiny HTTP server that answers `/health` with 200, for deploy tests
/// (blue/green health-gates the new slot before cutting over).
fn write_health_server(dir: &std::path::Path) -> String {
    let path = dir.join("healthapp.py");
    std::fs::write(
        &path,
        "#!/usr/bin/env python3\n\
         import os\n\
         from http.server import BaseHTTPRequestHandler, HTTPServer\n\
         class H(BaseHTTPRequestHandler):\n\
         \x20   def do_GET(self):\n\
         \x20       self.send_response(200)\n\
         \x20       self.send_header('Content-Length','2')\n\
         \x20       self.end_headers()\n\
         \x20       self.wfile.write(b'ok')\n\
         \x20   def log_message(self,*a): pass\n\
         HTTPServer(('127.0.0.1', int(os.environ.get('PORT','0'))), H).serve_forever()\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path.to_string_lossy().to_string()
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
                .uri(format!("/api/v1/orgs/default/apps/{id}/rollback"))
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
    let bin = write_health_server(dir.path());
    let id = create_app(&router, "dapp", &bin, 9201).await;

    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/default/apps/{id}/deploy"))
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
                .uri("/api/v1/orgs/default/apps")
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
                .uri(format!("/api/v1/orgs/default/apps/{id}/deploy"))
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
                .uri("/api/v1/orgs/default/apps")
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
    let bin = write_health_server(dir.path());
    let id = create_app(&router, "happ", &bin, 9600).await;

    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/default/apps/{id}/deploy"))
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
                .uri(format!("/api/v1/orgs/default/apps/{id}/deployments"))
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
                .uri(format!("/api/v1/orgs/default/deployments/{dep_id}"))
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
async fn env_crud_does_not_leak_values() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    let id = create_app(&router, "eapp", "/usr/bin/true", 9700).await;

    // invalid key
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/orgs/default/apps/{id}/env/1BAD"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"value": "x"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // set (fresh key → 201)
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/orgs/default/apps/{id}/env/API_KEY"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"value": "secret123"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // replace (existing key → 204)
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/orgs/default/apps/{id}/env/API_KEY"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"value": "secret456"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // list returns the key but never the value
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/orgs/default/apps/{id}/env"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["env"][0]["key"], "API_KEY");
    let text = body.to_string();
    assert!(!text.contains("secret123"));

    // delete
    let resp = router
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/orgs/default/apps/{id}/env/API_KEY"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn edge_routes_lists_apps_and_rejects_bad_token() {
    use crate::grpc::pb::EdgeRoutesRequest;

    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let router = app::build_router(state.clone());

    let payload = json!({
        "name": "edgeapp", "binary_path": "/usr/bin/true", "port": 9900, "domain": "app.test"
    });
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/apps")
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    assert!(crate::grpc::edge_routes(
        &state,
        EdgeRoutesRequest {
            token: "bad".into()
        }
    )
    .await
    .is_err());

    let resp = crate::grpc::edge_routes(
        &state,
        EdgeRoutesRequest {
            token: "test-join".into(),
        },
    )
    .await
    .unwrap();
    assert!(resp
        .routes
        .iter()
        .any(|r| r.host == "app.test" && r.address == "127.0.0.1" && r.port == 9900));
}

#[tokio::test]
async fn edge_certs_serves_cert_material() {
    use crate::grpc::pb::EdgeCertsRequest;

    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let router = app::build_router(state.clone());

    let payload = json!({
        "name": "capp", "binary_path": "/usr/bin/true", "port": 9910, "domain": "x.test"
    });
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/apps")
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let cert_dir = dir.path().join("certs/x.test");
    std::fs::create_dir_all(&cert_dir).unwrap();
    std::fs::write(cert_dir.join("fullchain.pem"), b"FULLCHAIN").unwrap();
    std::fs::write(cert_dir.join("privkey.pem"), b"PRIVKEY").unwrap();

    assert!(crate::grpc::edge_certs(
        &state,
        EdgeCertsRequest {
            token: "bad".into()
        }
    )
    .await
    .is_err());

    let resp = crate::grpc::edge_certs(
        &state,
        EdgeCertsRequest {
            token: "test-join".into(),
        },
    )
    .await
    .unwrap();
    let cert = resp.certs.iter().find(|c| c.host == "x.test").unwrap();
    assert_eq!(cert.fullchain_pem, b"FULLCHAIN");
    assert_eq!(cert.privkey_pem, b"PRIVKEY");
}

#[tokio::test]
async fn update_app_patches_fields() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    let id = create_app(&router, "uapp", "/usr/bin/true", 9800).await;

    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/orgs/default/apps/{id}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "port": 9801,
                        "domain": "u.test",
                        "runtime": "systemd",
                        "mem_limit_mb": 256,
                        "cpu_quota_pct": 50
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["application"]["port"], 9801);
    assert_eq!(body["application"]["domain"], "u.test");
    assert_eq!(body["application"]["mem_limit_mb"], 256);
    assert_eq!(body["application"]["cpu_quota_pct"], 50);

    // Omitted nullable fields preserve their current values.
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/orgs/default/apps/{id}"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"domain": "u.test"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["application"]["mem_limit_mb"], 256);
    assert_eq!(body["application"]["cpu_quota_pct"], 50);

    // Explicit null clears both limits to unlimited.
    let resp = router
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/orgs/default/apps/{id}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"mem_limit_mb": null, "cpu_quota_pct": null}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert!(body["application"]["mem_limit_mb"].is_null());
    assert!(body["application"]["cpu_quota_pct"].is_null());
}

#[tokio::test]
async fn stop_reports_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    let id = create_app(&router, "sapp", "/usr/bin/true", 9810).await;
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/default/apps/{id}/stop"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_json(resp).await["state"], "stopped");
}

#[tokio::test]
async fn rollback_to_explicit_build() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    let bin = write_health_server(dir.path());
    let id = create_app(&router, "rbapp", &bin, 9820).await;

    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/default/apps/{id}/deploy"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let hash = body_json(resp).await["artifact_hash"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/default/apps/{id}/rollback"))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "artifact_hash": hash }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_json(resp).await["rolled_back_to"], hash);
}

#[tokio::test]
async fn domains_crud() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    let id = create_app(&router, "dapp2", "/usr/bin/true", 9830).await;

    // invalid domain
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/default/apps/{id}/domains"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"domain": "bad"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // add
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/default/apps/{id}/domains"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"domain": "WWW.Example.COM"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(body_json(resp).await["domain"]["domain"], "www.example.com");

    // list
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/orgs/default/apps/{id}/domains"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        body_json(resp).await["domains"].as_array().unwrap().len(),
        1
    );

    // delete
    let resp = router
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!(
                    "/api/v1/orgs/default/apps/{id}/domains/www.example.com"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn logout_clears_session_cookie_with_path() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/logout")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let set_cookie = resp
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(set_cookie.contains("turaes_session="), "{set_cookie}");
    assert!(set_cookie.contains("Path=/"), "{set_cookie}");
}

#[tokio::test]
async fn callback_error_redirects_to_login() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    let resp = router
        .oneshot(
            Request::builder()
                .uri("/auth/callback?error=access_denied&error_description=denied")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
    let location = resp
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(location.starts_with("/?login_error="), "{location}");
}

#[tokio::test]
async fn deploy_uses_blue_green_slots() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    let bin = write_health_server(dir.path());
    let id = create_app(&router, "bgapp", &bin, 9250).await;

    let deploy = || {
        router.clone().oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/default/apps/{id}/deploy"))
                .body(Body::empty())
                .unwrap(),
        )
    };

    // First deploy -> slot A (base port).
    let resp = deploy().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let app = body_json(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/orgs/default/apps/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(app["application"]["active_port"], 9250);

    // Second deploy -> slot B (base + slot_offset = 1000).
    let resp = deploy().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let app = body_json(
        router
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/orgs/default/apps/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(app["application"]["active_port"], 10250);
}

#[tokio::test]
async fn deploy_health_failure_keeps_previous() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    // /usr/bin/true exits immediately and serves no health endpoint.
    let id = create_app(&router, "badapp", "/usr/bin/true", 9260).await;

    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/default/apps/{id}/deploy"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(!resp.status().is_success());

    let app = body_json(
        router
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/orgs/default/apps/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(app["application"]["status"], "failed");
    assert!(app["application"]["active_port"].is_null());
}

#[tokio::test]
async fn failed_redeploy_preserves_the_serving_slot_status() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let router = app::build_router(state.clone());
    let binary = write_health_server(dir.path());
    let id = create_app(&router, "keepapp", &binary, 9261).await;

    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/default/apps/{id}/deploy"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let active_before: i64 =
        sqlx::query_scalar("SELECT active_port FROM applications WHERE id = ?")
            .bind(&id)
            .fetch_one(&state.pool)
            .await
            .unwrap();

    // Replace the source with a process that exits instead of serving health.
    sqlx::query("UPDATE applications SET binary_path = '/usr/bin/true' WHERE id = ?")
        .bind(&id)
        .execute(&state.pool)
        .await
        .unwrap();
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/default/apps/{id}/deploy"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(!resp.status().is_success());

    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/orgs/default/apps/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let application = body_json(resp).await["application"].clone();
    assert_eq!(application["status"], "running");
    assert_eq!(application["active_port"], active_before);

    let app: turaes_core::models::Application =
        sqlx::query_as("SELECT * FROM applications WHERE id = ?")
            .bind(&id)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    let runtime = crate::routes::apps::runtime_for(&state.cfg, &app.runtime);
    runtime
        .stop(&crate::routes::apps::active_spec(&state.cfg, &app))
        .await
        .unwrap();
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
                .uri("/api/v1/orgs/default/apps")
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(resp).await;
    assert_eq!(body["field"], "name");
    assert_eq!(body["code"], "bad_request");
}

#[tokio::test]
async fn invalid_primary_domain_is_rejected_on_its_field() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    let payload = json!({
        "name": "badomain",
        "binary_path": "/bin/true",
        "port": 9370,
        "domain": "not a hostname",
    });
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/apps")
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body_json(resp).await["field"], "domain");
}

#[tokio::test]
async fn tenancy_schema_backfills_default_org() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let pool = state.pool.clone();

    // The seeded organization exists and is the backfill target.
    let slug: String = sqlx::query_scalar("SELECT slug FROM organizations WHERE id = 'default'")
        .fetch_one(&pool)
        .await
        .expect("default org seeded");
    assert_eq!(slug, "default");

    // New applications are tenant-scoped to the default org.
    sqlx::query(
        "INSERT INTO applications (id, name, binary_path, port) VALUES ('a1', 'demo', '/bin/true', 9000)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let org_id: String = sqlx::query_scalar("SELECT org_id FROM applications WHERE id = 'a1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(org_id, "default");

    // The tenancy tables accept the expected shapes.
    sqlx::query("INSERT INTO users (id, github_id, login) VALUES ('u1', 5284227, 'maulanasly')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO memberships (id, org_id, user_id, role) VALUES ('m1', 'default', 'u1', 'owner')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO api_tokens (id, org_id, user_id, name, token_hash, scopes) \
         VALUES ('t1', 'default', 'u1', 'ci', 'hash1', 'deploy')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO audit_log (id, org_id, actor_user_id, application_id, action) \
         VALUES ('l1', 'default', 'u1', 'a1', 'app.deploy')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let role: String = sqlx::query_scalar("SELECT role FROM memberships WHERE id = 'm1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(role, "owner");
    let action: String = sqlx::query_scalar("SELECT action FROM audit_log WHERE id = 'l1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(action, "app.deploy");
}

#[tokio::test]
async fn me_returns_resolved_principal_with_org_role() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let router = app::build_router(state.clone());

    let body = body_json(
        router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/me")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body["login"], "dev");
    assert_eq!(body["orgs"][0]["slug"], "default");
    assert_eq!(body["orgs"][0]["role"], "owner");

    // A second distinct user bootstraps as viewer, not owner.
    let second = crate::authz::resolve(&state, 12345, "second", None)
        .await
        .unwrap();
    assert_eq!(second.role_in("default"), Some(crate::authz::Role::Viewer));
    assert!(second
        .require("default", crate::authz::Role::Viewer)
        .is_ok());
    assert!(second
        .require("default", crate::authz::Role::Developer)
        .is_err());
}

#[tokio::test]
async fn cross_org_apps_are_invisible() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let router = app::build_router(state.clone());
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ('other', 'other', 'Other')")
        .execute(&state.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO applications (id, org_id, name, binary_path, port) \
         VALUES ('x1', 'other', 'otherapp', '/bin/true', 9500)",
    )
    .execute(&state.pool)
    .await
    .unwrap();

    // Another org's app is not listed and not fetchable (404, not 403).
    let body = body_json(
        router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/orgs/default/apps")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(body["applications"]
        .as_array()
        .unwrap()
        .iter()
        .all(|a| a["id"] != "x1"));

    let router = app::build_router(state.clone());
    let resp = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/orgs/default/apps/x1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // An org the caller is not a member of is indistinguishable from
    // an unknown org (both 404, so slugs cannot be enumerated)...
    let router = app::build_router(state.clone());
    let resp = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/orgs/other/apps")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // ...while an unknown org is not found.
    let router = app::build_router(state.clone());
    let resp = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/orgs/nope/apps")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn duplicate_and_paired_ports_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    let mk = |name: &str, port: u16| {
        let payload = json!({"name": name, "binary_path": "/bin/true", "port": port});
        Request::builder()
            .method("POST")
            .uri("/api/v1/orgs/default/apps")
            .header("content-type", "application/json")
            .body(Body::from(payload.to_string()))
            .unwrap()
    };
    let resp = router.oneshot(mk("papp", 9100)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Same port clashes.
    let router = test_router(dir.path()).await;
    let resp = router.oneshot(mk("qapp", 9100)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = body_json(resp).await;
    assert_eq!(body["field"], "port");
    assert_eq!(body["code"], "conflict");

    // The blue/green pair port (slot_offset is 1000 in tests) clashes too.
    let router = test_router(dir.path()).await;
    let resp = router.oneshot(mk("rapp", 10100)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn role_floors_gate_org_access() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    // Bootstrap the owner first so the subject below lands as viewer.
    crate::authz::resolve(&state, 0, "dev", Some("Dev mode"))
        .await
        .unwrap();
    let viewer = crate::authz::resolve(&state, 777001, "floored", None)
        .await
        .unwrap();
    assert_eq!(viewer.role_in("default"), Some(crate::authz::Role::Viewer));
    // Viewers may read but not administer or operate the fleet.
    crate::authz::authorize_org(&state, &viewer, "default", crate::authz::Role::Viewer)
        .await
        .unwrap();
    let err = crate::authz::authorize_org(&state, &viewer, "default", crate::authz::Role::Admin)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("forbidden"));
    assert!(crate::authz::require_operator(&viewer).is_err());
}

#[tokio::test]
async fn audit_records_mutations_without_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;

    let payload = json!({"name": "auditapp", "binary_path": "/bin/true", "port": 9200});
    let created = body_json(
        router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/default/apps")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let id = created["application"]["id"].as_str().unwrap().to_string();

    let router = test_router(dir.path()).await;
    let secret = "super-secret-value";
    let resp = router
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/orgs/default/apps/{id}/env/API_KEY"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"value": secret}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let router = test_router(dir.path()).await;
    let body = body_json(
        router
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/orgs/default/audit?app={id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let rows = body["audit"].as_array().unwrap();
    let actions: Vec<&str> = rows.iter().map(|r| r["action"].as_str().unwrap()).collect();
    assert!(actions.contains(&"app.create"));
    assert!(actions.contains(&"env.set"));
    // Actor attribution resolves to the dev login.
    assert!(rows.iter().all(|r| r["actor_login"] == "dev"));
    // The secret value never lands in the audit trail.
    let dump = serde_json::to_string(rows).unwrap();
    assert!(!dump.contains(secret));
    let env_row = rows.iter().find(|r| r["action"] == "env.set").unwrap();
    assert!(env_row["metadata"].as_str().unwrap().contains("API_KEY"));
}

#[tokio::test]
async fn audit_is_org_scoped() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ('other', 'other', 'Other')")
        .execute(&state.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO audit_log (id, org_id, action) VALUES ('lx', 'other', 'app.create')")
        .execute(&state.pool)
        .await
        .unwrap();

    // The other org's rows are invisible from default...
    let router = app::build_router(state.clone());
    let body = body_json(
        router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/orgs/default/audit")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(body["audit"].as_array().unwrap().is_empty());

    // ...and the other org itself is indistinguishable from unknown.
    let router = app::build_router(state.clone());
    let resp = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/orgs/other/audit")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

async fn mint_token(router: axum::Router, scopes: &str) -> (String, String) {
    let body = body_json(
        router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/default/tokens")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"name": format!("ci-{scopes}"), "scopes": scopes}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body["token"]["scopes"], scopes);
    // The hash is never serialized; the plaintext is returned exactly once.
    assert!(body["token"].get("token_hash").is_none());
    let plaintext = body["plaintext"].as_str().unwrap().to_string();
    assert!(plaintext.starts_with("turaes_"));
    (body["token"]["id"].as_str().unwrap().to_string(), plaintext)
}

fn bearer(uri: &str, plaintext: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("authorization", format!("Bearer {plaintext}"))
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn token_lifecycle_and_scope_floors() {
    let dir = tempfile::tempdir().unwrap();
    let (_, read_token) = mint_token(test_router(dir.path()).await, "read").await;
    let (_, deploy_token) = mint_token(test_router(dir.path()).await, "deploy").await;

    // Read scope: lists fine, creating is forbidden, fleet is forbidden.
    let router = test_router(dir.path()).await;
    let resp = router
        .oneshot(bearer("/api/v1/orgs/default/apps", &read_token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let router = test_router(dir.path()).await;
    let resp = router
        .oneshot(bearer("/api/v1/orgs/default/apps/nope/stop", &read_token))
        .await
        .unwrap();
    // Wrong method for the stop path (GET vs POST) aside, authz runs first.
    assert!(
        resp.status() == StatusCode::FORBIDDEN || resp.status() == StatusCode::METHOD_NOT_ALLOWED
    );
    let router = test_router(dir.path()).await;
    let resp = router
        .oneshot(bearer("/api/v1/servers", &read_token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Deploy scope passes the developer floor (404: the app itself is missing).
    let router = test_router(dir.path()).await;
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/apps/nope/stop")
                .header("authorization", format!("Bearer {deploy_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Revocation kills the token.
    let (id, _) = mint_token(test_router(dir.path()).await, "read").await;
    let router = test_router(dir.path()).await;
    let resp = router
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/orgs/default/tokens/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // A revoked token's plaintext no longer authenticates (mint a fresh one
    // and revoke it to capture the plaintext before it dies).
    let (rid, rplain) = mint_token(test_router(dir.path()).await, "read").await;
    let router = test_router(dir.path()).await;
    router
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/orgs/default/tokens/{rid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let router = test_router(dir.path()).await;
    let resp = router
        .oneshot(bearer("/api/v1/orgs/default/apps", &rplain))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn token_rejects_unknown_and_cross_org() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let (_, token) = mint_token(app::build_router(state.clone()), "admin").await;

    // Unknown tokens are unauthorized.
    let router = app::build_router(state.clone());
    let resp = router
        .oneshot(bearer("/api/v1/orgs/default/apps", "bogus"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // A token is bound to its org: another org reads as unknown.
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ('other', 'other', 'Other')")
        .execute(&state.pool)
        .await
        .unwrap();
    let router = app::build_router(state.clone());
    let resp = router
        .oneshot(bearer("/api/v1/orgs/other/apps", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Invalid scopes are rejected at mint time.
    let router = app::build_router(state.clone());
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/tokens")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"name": "x", "scopes": "owner"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

async fn post_json(
    router: axum::Router,
    uri: &str,
    payload: serde_json::Value,
) -> axum::response::Response {
    router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn org_create_and_membership_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;

    let resp = post_json(
        app::build_router(state.clone()),
        "/api/v1/orgs",
        json!({"slug": "acme", "name": "Acme"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Duplicate slugs conflict; malformed slugs are rejected.
    let router = app::build_router(state.clone());
    let resp = post_json(router, "/api/v1/orgs", json!({"slug": "acme"})).await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let router = app::build_router(state.clone());
    let resp = post_json(router, "/api/v1/orgs", json!({"slug": "Bad Slug!"})).await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // The creator is the founding owner.
    let router = app::build_router(state.clone());
    let body = body_json(
        router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/orgs/acme/members")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let members = body["members"].as_array().unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0]["login"], "dev");
    assert_eq!(members[0]["role"], "owner");
    let dev_uid = members[0]["user_id"].as_str().unwrap().to_string();

    // Invite by raw GitHub id, then re-invite conflicts.
    let router = app::build_router(state.clone());
    let resp = post_json(
        router,
        "/api/v1/orgs/acme/members",
        json!({"github_id": 424242, "role": "developer"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let router = app::build_router(state.clone());
    let resp = post_json(
        router,
        "/api/v1/orgs/acme/members",
        json!({"github_id": 424242}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // Promote, then the sole-owner guards engage.
    // (PATCH needs a member id; fetch it from the roster.)
    let router = app::build_router(state.clone());
    let body = body_json(
        router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/orgs/acme/members")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let other = body["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["login"] != "dev")
        .unwrap();
    let other_uid = other["user_id"].as_str().unwrap().to_string();

    let router = app::build_router(state.clone());
    let resp = router
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/orgs/acme/members/{other_uid}"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"role": "admin"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Removing the sole owner is refused...
    let router = app::build_router(state.clone());
    let resp = router
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/orgs/acme/members/{dev_uid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // ...until a second owner exists.
    let router = app::build_router(state.clone());
    let resp = router
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/orgs/acme/members/{other_uid}"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"role": "owner"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let router = app::build_router(state.clone());
    let resp = router
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/orgs/acme/members/{dev_uid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Removed members lose access entirely (unknown org, not a guard trip).
    let router = app::build_router(state.clone());
    let resp = router
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/orgs/acme/members/{other_uid}"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"role": "viewer"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // The sole-owner demote guard trips where the caller is still a member.
    let router = app::build_router(state.clone());
    let resp = router
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/orgs/default/members/{dev_uid}"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"role": "viewer"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn member_management_requires_owner() {
    let dir = tempfile::tempdir().unwrap();
    let (_, read_token) = mint_token(test_router(dir.path()).await, "read").await;

    // Viewers may see the roster but not change it.
    let router = test_router(dir.path()).await;
    let resp = router
        .oneshot(bearer("/api/v1/orgs/default/members", &read_token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let router = test_router(dir.path()).await;
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/members")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {read_token}"))
                .body(Body::from(json!({"github_id": 111}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn secrets_reseal_migrates_legacy_only() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    sqlx::query(
        "INSERT INTO applications (id, name, binary_path, port) \
         VALUES ('s1', 'sres', '/bin/true', 9300)",
    )
    .execute(&state.pool)
    .await
    .unwrap();
    let legacy = state.secrets.seal_legacy("old-value").unwrap();
    let current = state.secrets.seal("new-value").unwrap();
    sqlx::query(
        "INSERT INTO env_vars (id, application_id, key, value_enc) VALUES ('e1', 's1', 'A', ?)",
    )
    .bind(&legacy)
    .execute(&state.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO env_vars (id, application_id, key, value_enc) VALUES ('e2', 's1', 'B', ?)",
    )
    .bind(&current)
    .execute(&state.pool)
    .await
    .unwrap();

    let (up, total, ssh_up, ssh_total) = crate::secrets::reseal(&state).await.unwrap();
    assert_eq!((up, total), (1, 2));
    assert_eq!((ssh_up, ssh_total), (0, 0));

    // Values intact and everything is v1 now; a second run is a no-op.
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT key, value_enc FROM env_vars ORDER BY key")
            .fetch_all(&state.pool)
            .await
            .unwrap();
    for (key, sealed) in &rows {
        assert!(!turaes_core::crypto::SecretBox::is_legacy_format(sealed));
        let want = if key == "A" { "old-value" } else { "new-value" };
        assert_eq!(state.secrets.open(sealed).unwrap(), want);
    }
    let (up2, _, _, _) = crate::secrets::reseal(&state).await.unwrap();
    assert_eq!(up2, 0);

    // The run itself is audited.
    let action: String = sqlx::query_scalar(
        "SELECT action FROM audit_log WHERE action = 'secrets.reseal' ORDER BY rowid DESC LIMIT 1",
    )
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert_eq!(action, "secrets.reseal");
}

#[tokio::test]
async fn security_headers_are_present() {
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
    let headers = resp.headers();
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(headers["x-frame-options"], "DENY");
    assert_eq!(headers["referrer-policy"], "same-origin");
    // The test origin is http://localhost, which counts as secure.
    assert_eq!(headers["strict-transport-security"], "max-age=31536000");
}

#[tokio::test]
async fn csrf_blocks_cookie_mutations_without_json() {
    let dir = tempfile::tempdir().unwrap();
    // A cross-site form POST carries cookies but no JSON content type.
    let router = test_router(dir.path()).await;
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/apps")
                .header("cookie", "turaes_session=junk")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // The same request as a real API call (JSON content type) passes CSRF and
    // reaches the handler (the dev bypass authenticates in tests).
    let router = test_router(dir.path()).await;
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/apps")
                .header("cookie", "turaes_session=junk")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"name": "csrfapp", "binary_path": "/bin/true", "port": 9400})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn limits_stored_and_validated() {
    let dir = tempfile::tempdir().unwrap();
    // systemd runtime accepts limits.
    let router = test_router(dir.path()).await;
    let payload = json!({
        "name": "limited", "binary_path": "/bin/true", "port": 9500,
        "runtime": "systemd", "mem_limit_mb": 256, "cpu_quota_pct": 50,
    });
    let body = body_json(
        router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/default/apps")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body["application"]["mem_limit_mb"], 256);
    assert_eq!(body["application"]["cpu_quota_pct"], 50);

    // Out-of-range values are rejected.
    for (bad, field) in [
        (
            json!({"name": "b1", "binary_path": "/bin/true", "port": 9501, "mem_limit_mb": 8}),
            "mem_limit_mb",
        ),
        (
            json!({"name": "b2", "binary_path": "/bin/true", "port": 9502, "cpu_quota_pct": 0}),
            "cpu_quota_pct",
        ),
    ] {
        let router = test_router(dir.path()).await;
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/default/apps")
                    .header("content-type", "application/json")
                    .body(Body::from(bad.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body_json(resp).await["field"], field);
    }

    // The proc runtime cannot confine: limits with it are rejected, not
    // silently ignored (tests default to the proc driver).
    let router = test_router(dir.path()).await;
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/apps")
                .header("content-type", "application/json")
                .body(
                    Body::from(
                        json!({"name": "b3", "binary_path": "/bin/true", "port": 9503, "mem_limit_mb": 256})
                            .to_string(),
                    ),
                )
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body_json(resp).await["field"], "runtime");
}

#[tokio::test]
async fn quota_enforcement() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let mk = |name: &str, port: u16, extra: serde_json::Value| {
        let mut payload = json!({"name": name, "binary_path": "/bin/true", "port": port});
        for (k, v) in extra.as_object().unwrap() {
            payload[k] = v.clone();
        }
        Request::builder()
            .method("POST")
            .uri("/api/v1/orgs/default/apps")
            .header("content-type", "application/json")
            .body(Body::from(payload.to_string()))
            .unwrap()
    };

    // Cap the org at one app.
    let router = app::build_router(state.clone());
    let resp = router
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/orgs/default/quota")
                .header("content-type", "application/json")
                .body(Body::from(json!({"max_apps": 1}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let router = app::build_router(state.clone());
    let resp = router.oneshot(mk("q1", 9510, json!({}))).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let router = app::build_router(state.clone());
    let resp = router.oneshot(mk("q2", 9511, json!({}))).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // Memory budget: a 256MB app does not fit in 100MB, but an unlimited
    // app consumes no budget (it still counts toward max_apps, raised first).
    let router = app::build_router(state.clone());
    let resp = router
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/orgs/default/quota")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"max_apps": 20, "max_mem_mb": 100}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let router = app::build_router(state.clone());
    let resp = router
        .oneshot(mk(
            "q3",
            9512,
            json!({"runtime": "systemd", "mem_limit_mb": 256}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // Hostname budget: zero means no new claims.
    let router = app::build_router(state.clone());
    let resp = router
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/orgs/default/quota")
                .header("content-type", "application/json")
                .body(Body::from(json!({"max_domains": 0}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let app_id: String = sqlx::query_scalar("SELECT id FROM applications WHERE name = 'q1'")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    let router = app::build_router(state.clone());
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/default/apps/{app_id}/domains"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"domain": "blocked.example.com"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn quota_get_reports_usage() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let router = app::build_router(state.clone());
    router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/apps")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "name": "usageapp", "binary_path": "/bin/true", "port": 9520,
                        "runtime": "systemd", "mem_limit_mb": 128,
                        "domain": "usage.example.com",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    let router = app::build_router(state.clone());
    let body = body_json(
        router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/orgs/default/quota")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body["usage"]["apps"], 1);
    assert_eq!(body["usage"]["mem_mb"], 128);
    assert_eq!(body["usage"]["domains"], 1);
    assert!(body["quota"]["max_apps"].as_i64().unwrap() >= 1);
}

fn age_file(path: &std::path::Path, hours_ago: u64) {
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(hours_ago * 3600);
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(old)
        .unwrap();
}

#[tokio::test]
async fn artifact_gc_keeps_referenced_blobs() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let live_hex = "a".repeat(64);
    let orphan_hex = "b".repeat(64);
    state
        .artifacts
        .store_bytes(&format!("sha256:{live_hex}"), b"live")
        .await
        .unwrap();
    let orphan = state
        .artifacts
        .store_bytes(&format!("sha256:{orphan_hex}"), b"orphan")
        .await
        .unwrap();
    // Only blobs older than the grace period are collectible.
    age_file(&orphan, 2);

    // A stale crashed upload is swept; a fresh one is spared.
    let sha_dir = state.artifacts.root().join("sha256");
    std::fs::write(sha_dir.join(".tmp-stale"), b"x").unwrap();
    age_file(&sha_dir.join(".tmp-stale"), 2);
    std::fs::write(sha_dir.join(".tmp-fresh"), b"x").unwrap();

    sqlx::query(
        "INSERT INTO applications (id, name, binary_path, port) VALUES ('g1', 'gcapp', '/bin/true', 9600)",
    )
    .execute(&state.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO deployments (id, application_id, status, artifact_hash) VALUES ('gd', 'g1', 'running', ?)",
    )
    .bind(format!("sha256:{live_hex}"))
    .execute(&state.pool)
    .await
    .unwrap();

    // Dry run changes nothing.
    let dry = crate::gc::collect_artifacts(&state, true).await.unwrap();
    assert_eq!(dry.blobs_removed, 1);
    assert_eq!(dry.tmps_swept, 1);
    assert_eq!(dry.blobs_kept, 1);
    assert!(orphan.is_file());

    let report = crate::gc::collect_artifacts(&state, false).await.unwrap();
    assert_eq!(report.blobs_removed, 1);
    assert!(report.bytes_freed > 0);
    assert_eq!(report.tmps_swept, 1);
    assert!(!orphan.is_file());
    assert!(!sha_dir.join(".tmp-stale").exists());
    assert!(sha_dir.join(".tmp-fresh").is_file());
    assert!(state.artifacts.has(&format!("sha256:{live_hex}")));

    // The collection itself is audited.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = 'artifacts.gc'")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn alerts_fire_and_resolve_for_unhealthy_app() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    sqlx::query(
        "INSERT INTO applications (id, name, binary_path, port, status) \
         VALUES ('al1', 'alertapp', '/bin/true', 9700, 'unhealthy')",
    )
    .execute(&state.pool)
    .await
    .unwrap();

    crate::alerts::evaluate(&state).await;
    let body = body_json(
        app::build_router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/v1/orgs/default/alerts")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let rows = body["alerts"].as_array().unwrap();
    let unl = rows.iter().find(|r| r["kind"] == "app.unhealthy").unwrap();
    assert_eq!(unl["subject"], "alertapp is unhealthy");
    assert!(unl["notified_at"].is_null());

    // Evaluation is idempotent: no duplicate firing rows.
    crate::alerts::evaluate(&state).await;
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM alerts WHERE kind = 'app.unhealthy' AND status = 'firing'",
    )
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert_eq!(n, 1);

    // Recovery resolves; history is queryable.
    sqlx::query("UPDATE applications SET status = 'running' WHERE id = 'al1'")
        .execute(&state.pool)
        .await
        .unwrap();
    crate::alerts::evaluate(&state).await;
    let router = app::build_router(state.clone());
    let body = body_json(
        router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/orgs/default/alerts?status=resolved")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(body["alerts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["kind"] == "app.unhealthy"));
}

#[tokio::test]
async fn alerts_fire_for_failed_deploy_until_superseded() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    sqlx::query(
        "INSERT INTO applications (id, name, binary_path, port, status) \
         VALUES ('al2', 'failapp', '/bin/true', 9701, 'failed')",
    )
    .execute(&state.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO deployments (id, application_id, status, finished_at) \
         VALUES ('fd1', 'al2', 'failed', datetime('now'))",
    )
    .execute(&state.pool)
    .await
    .unwrap();

    crate::alerts::evaluate(&state).await;
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM alerts WHERE kind = 'deploy.failed' AND status = 'firing'",
    )
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert_eq!(n, 1);

    // A newer success resolves it.
    sqlx::query(
        "INSERT INTO deployments (id, application_id, status, finished_at) \
         VALUES ('ok1', 'al2', 'running', datetime('now'))",
    )
    .execute(&state.pool)
    .await
    .unwrap();
    crate::alerts::evaluate(&state).await;
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM alerts WHERE kind = 'deploy.failed' AND status = 'firing'",
    )
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn alerts_backup_stale_resolves_on_fresh_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;

    // Empty backup dir: platform alert fires (visible to org admins).
    crate::alerts::evaluate(&state).await;
    let router = app::build_router(state.clone());
    let body = body_json(
        router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/orgs/default/alerts")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let stale = body["alerts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["kind"] == "backup.stale")
        .unwrap()
        .clone();
    assert_eq!(stale["severity"], "critical");
    assert!(stale["org_id"].is_null());

    // A fresh snapshot resolves it.
    let backup_dir = dir.path().join("backups");
    std::fs::create_dir_all(&backup_dir).unwrap();
    std::fs::write(backup_dir.join("turaes-test.db"), b"fake").unwrap();
    crate::alerts::evaluate(&state).await;
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM alerts WHERE kind = 'backup.stale' AND status = 'firing'",
    )
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert_eq!(n, 0);

    // Manual resolve of a still-firing alert works and is audited.
    sqlx::query(
        "INSERT INTO applications (id, name, binary_path, port, status) \
         VALUES ('al3', 'ackapp', '/bin/true', 9702, 'unhealthy')",
    )
    .execute(&state.pool)
    .await
    .unwrap();
    crate::alerts::evaluate(&state).await;
    let id: String = sqlx::query_scalar(
        "SELECT id FROM alerts WHERE kind = 'app.unhealthy' AND status = 'firing'",
    )
    .fetch_one(&state.pool)
    .await
    .unwrap();
    let router = app::build_router(state.clone());
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/default/alerts/{id}/resolve"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = 'alert.resolve'")
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(n, 1);

    // Resolving twice is a 404 (nothing firing under that id).
    let router = app::build_router(state.clone());
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/default/alerts/{id}/resolve"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn cli_org_scoping_and_remove() {
    use crate::cli::AppCommand;

    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;

    // Unknown orgs are rejected, not silently defaulted.
    let err = crate::commands::run(&state, AppCommand::List, Some("nope"), false)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("unknown organization"));

    // Add + show + remove round-trip inside the default org.
    crate::commands::run(
        &state,
        AppCommand::Add {
            name: "cliapp".into(),
            binary: Some("/bin/true".into()),
            command: None,
            workdir: None,
            publish_dir: None,
            kind: "service".into(),
            port: Some(9800),
            domain: None,
            health: "/health".into(),
            metrics: "/metrics".into(),
            runtime: "proc".into(),
            args: None,
        },
        Some("default"),
        false,
    )
    .await
    .unwrap();
    crate::commands::run(
        &state,
        AppCommand::Show {
            name: "cliapp".into(),
        },
        Some("default"),
        true,
    )
    .await
    .unwrap();

    // A second org cannot see the app by name.
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ('other', 'other', 'Other')")
        .execute(&state.pool)
        .await
        .unwrap();
    let err = crate::commands::run(
        &state,
        AppCommand::Show {
            name: "cliapp".into(),
        },
        Some("other"),
        false,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("no application named"));

    crate::commands::run(
        &state,
        AppCommand::Remove {
            name: "cliapp".into(),
        },
        Some("default"),
        false,
    )
    .await
    .unwrap();
    let err = crate::commands::run(
        &state,
        AppCommand::Show {
            name: "cliapp".into(),
        },
        Some("default"),
        false,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("no application named"));
}

#[tokio::test]
async fn doctor_passes_and_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}/test.db?mode=rwc", dir.path().display());
    let cfg = test_config(dir.path(), &url);
    crate::doctor(&cfg, true).await.unwrap();

    let mut bad = test_config(dir.path(), &url);
    bad.database.url = "sqlite:///proc/definitely-not-here/t.db?mode=rwc".into();
    assert!(crate::doctor(&bad, true).await.is_err());
}

#[tokio::test]
async fn errors_carry_stable_codes() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    let resp = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/orgs/default/apps/nope")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = body_json(resp).await;
    assert_eq!(body["code"], "not_found");
    assert!(body["detail"].as_str().unwrap().contains("nope"));

    let router = test_router(dir.path()).await;
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/apps")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"name": "Bad Name", "binary_path": "/bin/true", "port": 1}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(resp).await;
    assert_eq!(body["code"], "bad_request");
}

#[tokio::test]
async fn list_limit_caps_rows() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    for (name, port) in [("lim1", 9901), ("lim2", 9902), ("lim3", 9903)] {
        let payload = json!({"name": name, "binary_path": "/bin/true", "port": port});
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/default/apps")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
    }

    let router = test_router(dir.path()).await;
    let body = body_json(
        router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/orgs/default/apps?limit=2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body["applications"].as_array().unwrap().len(), 2);

    // Absent limit still returns everything (dashboard behavior preserved).
    let router = test_router(dir.path()).await;
    let body = body_json(
        router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/orgs/default/apps")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body["applications"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn cli_secrets_set_unset_roundtrip() {
    use crate::cli::SecretsCommand;

    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    sqlx::query(
        "INSERT INTO applications (id, name, binary_path, port) \
         VALUES ('sec1', 'secapp', '/bin/true', 9910)",
    )
    .execute(&state.pool)
    .await
    .unwrap();

    crate::commands::run_secrets(
        &state,
        SecretsCommand::Set {
            app: "secapp".into(),
            key: "API_KEY".into(),
            value: Some("v1".into()),
        },
        Some("default"),
    )
    .await
    .unwrap();
    let sealed: String = sqlx::query_scalar(
        "SELECT value_enc FROM env_vars WHERE application_id = 'sec1' AND key = 'API_KEY'",
    )
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert_eq!(state.secrets.open(&sealed).unwrap(), "v1");

    crate::commands::run_secrets(
        &state,
        SecretsCommand::Unset {
            app: "secapp".into(),
            key: "API_KEY".into(),
        },
        Some("default"),
    )
    .await
    .unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM env_vars WHERE application_id = 'sec1'")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(n, 0);

    // Unsetting again is a clean NotFound, not a silent no-op.
    let err = crate::commands::run_secrets(
        &state,
        SecretsCommand::Unset {
            app: "secapp".into(),
            key: "API_KEY".into(),
        },
        Some("default"),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("no secret"));
}

async fn apply_manifest(
    state: &AppState,
    org_id: &str,
    yaml: &str,
    base: &std::path::Path,
    dry_run: bool,
) -> turaes_core::Result<crate::apply::ApplyReport> {
    let manifest = turaes_core::manifest::AppManifest::parse(yaml).unwrap();
    crate::apply::apply_manifest(state, org_id, &manifest, base, dry_run).await
}

#[tokio::test]
async fn apply_creates_updates_and_dry_runs() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let yaml = r#"
name: yamlapp
binary: /bin/true
port: 9920
domain: yaml.example.com
aliases: [www.yaml.example.com]
env:
  LOG_LEVEL: info
"#;

    // Dry run first: nothing written.
    let report = apply_manifest(&state, "default", yaml, dir.path(), true)
        .await
        .unwrap();
    assert!(report.created);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM applications WHERE name = 'yamlapp'")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(n, 0);

    // Real apply creates everything.
    let report = apply_manifest(&state, "default", yaml, dir.path(), false)
        .await
        .unwrap();
    assert!(report.created);
    let app: turaes_core::models::Application =
        sqlx::query_as("SELECT * FROM applications WHERE name = 'yamlapp'")
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(app.port, 9920);
    assert_eq!(app.kind, "service");
    let placements: i64 =
        sqlx::query_scalar("SELECT count(*) FROM app_servers WHERE application_id = ?")
            .bind(&app.id)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(placements, 1);
    let health_path: String =
        sqlx::query_scalar("SELECT path FROM health_checks WHERE application_id = ?")
            .bind(&app.id)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(health_path, "/health");
    let aliases: i64 = sqlx::query_scalar("SELECT count(*) FROM domains WHERE application_id = ?")
        .bind(&app.id)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(aliases, 1);
    let env_val: String = sqlx::query_scalar(
        "SELECT value_enc FROM env_vars WHERE application_id = ? AND key = 'LOG_LEVEL'",
    )
    .bind(&app.id)
    .fetch_one(&state.pool)
    .await
    .map(|sealed: String| state.secrets.open(&sealed).unwrap())
    .unwrap();
    assert_eq!(env_val, "info");

    // Re-apply is a no-op.
    let report = apply_manifest(&state, "default", yaml, dir.path(), false)
        .await
        .unwrap();
    assert!(!report.created && report.changed.is_empty());

    // Port change shows up as a diff and sticks.
    let yaml2 = yaml.replace("port: 9920", "port: 9921");
    let report = apply_manifest(&state, "default", &yaml2, dir.path(), false)
        .await
        .unwrap();
    assert!(report.changed.contains(&"port".to_string()));
    let port: i64 = sqlx::query_scalar("SELECT port FROM applications WHERE name = 'yamlapp'")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(port, 9921);
}

#[tokio::test]
async fn apply_aborts_on_missing_secret_and_prunes() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let yaml = r#"
name: secapp
binary: /bin/true
port: 9930
secrets: [DATABASE_URL]
"#;
    // First apply aborts on the missing secret — but leaves the shell app so
    // `secrets set` has something to attach to (retry completes it).
    let err = apply_manifest(&state, "default", yaml, dir.path(), false)
        .await
        .unwrap_err();
    assert!(err
        .to_string()
        .contains("turaes secrets set secapp DATABASE_URL"));
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM applications WHERE name = 'secapp'")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(n, 1);

    crate::commands::run_secrets(
        &state,
        crate::cli::SecretsCommand::Set {
            app: "secapp".into(),
            key: "DATABASE_URL".into(),
            value: Some("postgres://db".into()),
        },
        Some("default"),
    )
    .await
    .unwrap();
    let report = apply_manifest(&state, "default", yaml, dir.path(), false)
        .await
        .unwrap();
    assert!(!report.created);
    let stored: String = sqlx::query_scalar(
        "SELECT value_enc FROM env_vars WHERE key = 'DATABASE_URL' AND application_id = (SELECT id FROM applications WHERE name = 'secapp')",
    )
    .fetch_one(&state.pool)
    .await
    .map(|sealed: String| state.secrets.open(&sealed).unwrap())
    .unwrap();
    assert_eq!(stored, "postgres://db");

    // Prune check: add a stale key + alias outside the file, re-apply, gone.
    let app_id: String = sqlx::query_scalar("SELECT id FROM applications WHERE name = 'secapp'")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO env_vars (id, application_id, key, value_enc) VALUES ('stale', ?, 'OLD_KEY', 'x')")
        .bind(&app_id)
        .execute(&state.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO domains (id, application_id, domain) VALUES ('stdom', ?, 'stale.example.com')",
    )
    .bind(&app_id)
    .execute(&state.pool)
    .await
    .unwrap();
    let report = apply_manifest(&state, "default", yaml, dir.path(), false)
        .await
        .unwrap();
    assert_eq!(report.pruned_env, 1);
    assert_eq!(report.pruned_aliases, 1);
    // ...while the declared secret survives.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM env_vars WHERE key = 'DATABASE_URL'")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn api_rejects_kind_mismatches() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    // Worker with a port.
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/apps")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"name": "w1", "binary_path": "/bin/sleep", "port": 1, "kind": "worker"})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Static without publish_dir.
    let router = test_router(dir.path()).await;
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/apps")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"name": "s1", "port": 9941, "kind": "static"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // binary + command together (/usr/bin/true exists on macOS and Linux).
    let router = test_router(dir.path()).await;
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/apps")
                .header("content-type", "application/json")
                .body(
                    Body::from(
                        json!({"name": "c1", "binary_path": "/usr/bin/true", "command": ["/usr/bin/true"], "port": 9942})
                            .to_string(),
                    ),
                )
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn preflight_collects_every_problem_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    // Occupy a port first (tests default to the proc driver, so the memory
    // limit below is also rejected: three independent problems).
    create_app(&router, "taken", "/bin/true", 9600).await;

    let payload = json!({
        "name": "Bad Name!",
        "binary_path": "/bin/true",
        "port": 9600,
        "mem_limit_mb": 256,
    });
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/apps/preflight")
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["ok"], false);
    let fields: Vec<&str> = body["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["field"].as_str().unwrap())
        .collect();
    assert!(
        fields.contains(&"name"),
        "expected a name error: {fields:?}"
    );
    assert!(
        fields.contains(&"port"),
        "expected a port error: {fields:?}"
    );
    assert!(
        fields.contains(&"runtime"),
        "expected a runtime error: {fields:?}"
    );
    assert_eq!(body["checks"]["port"], "fail");
    // A service without a domain warns instead of failing.
    let warnings = body["warnings"].as_array().unwrap();
    assert!(
        warnings.iter().any(|w| w["field"] == "domain"),
        "expected a domain warning"
    );
}

#[tokio::test]
async fn preflight_edit_context_excludes_self() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    let id = create_app(&router, "selfone", "/bin/true", 9610).await;

    // Previewing the unchanged port must not clash with itself.
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/apps/preflight")
                .header("content-type", "application/json")
                .body(Body::from(json!({"app_id": id, "port": 9610}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["ok"], true);
    assert!(body["errors"].as_array().unwrap().is_empty());

    // ...but another app's port still clashes.
    create_app(&router, "selftwo", "/bin/true", 9611).await;
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/apps/preflight")
                .header("content-type", "application/json")
                .body(Body::from(json!({"app_id": id, "port": 9611}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["ok"], false);
    assert_eq!(body["errors"][0]["field"], "port");
}

#[tokio::test]
async fn duplicate_primary_domain_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    let mk = |name: &str, port: u16, domain: &str| {
        let payload =
            json!({"name": name, "binary_path": "/bin/true", "port": port, "domain": domain});
        Request::builder()
            .method("POST")
            .uri("/api/v1/orgs/default/apps")
            .header("content-type", "application/json")
            .body(Body::from(payload.to_string()))
            .unwrap()
    };
    let resp = router
        .clone()
        .oneshot(mk("domone", 9620, "d1.test"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = router.oneshot(mk("domtwo", 9621, "D1.TEST")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = body_json(resp).await;
    assert_eq!(body["field"], "domain");
    assert_eq!(body["code"], "conflict");
}

#[tokio::test]
async fn alias_conflicting_with_primary_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    let mk = |name: &str, port: u16| {
        let payload = json!({"name": name, "binary_path": "/bin/true", "port": port});
        Request::builder()
            .method("POST")
            .uri("/api/v1/orgs/default/apps")
            .header("content-type", "application/json")
            .body(Body::from(payload.to_string()))
            .unwrap()
    };
    // App A owns the primary.
    let resp = router.clone().oneshot(mk("aliasone", 9630)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let a = body_json(resp).await["application"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/orgs/default/apps/{a}"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"domain": "shared.test"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // App B cannot take it as an alias.
    let resp = router.clone().oneshot(mk("aliastwo", 9631)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let b = body_json(resp).await["application"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/default/apps/{b}/domains"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"domain": "shared.test"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert_eq!(body_json(resp).await["field"], "domain");

    // ...nor can A alias its own primary.
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/default/apps/{a}/domains"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"domain": "shared.test"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert_eq!(body_json(resp).await["field"], "domain");
}

#[tokio::test]
async fn lifecycle_stop_start_restart_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    let bin = write_health_server(dir.path());
    let id = create_app(&router, "cycleapp", &bin, 9640).await;
    let act = |method: &str, path: String| {
        router.clone().oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .body(Body::empty())
                .unwrap(),
        )
    };
    let status_of = |router: &axum::Router| {
        let id = id.clone();
        router.clone().oneshot(
            Request::builder()
                .uri(format!("/api/v1/orgs/default/apps/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
    };

    let resp = act("POST", format!("/api/v1/orgs/default/apps/{id}/deploy"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = act("POST", format!("/api/v1/orgs/default/apps/{id}/restart"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let app = body_json(status_of(&router).await.unwrap()).await;
    assert_eq!(app["application"]["status"], "running");

    let resp = act("POST", format!("/api/v1/orgs/default/apps/{id}/stop"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let app = body_json(status_of(&router).await.unwrap()).await;
    assert_eq!(app["application"]["status"], "stopped");

    let resp = act("POST", format!("/api/v1/orgs/default/apps/{id}/start"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let app = body_json(status_of(&router).await.unwrap()).await;
    assert_eq!(app["application"]["status"], "running");
}

#[tokio::test]
async fn worker_restart_reports_running() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    let payload =
        json!({"name": "sleeprestart", "kind": "worker", "command": ["/bin/sleep", "60"]});
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/apps")
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

    for action in ["deploy", "restart"] {
        let resp = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/orgs/default/apps/{id}/{action}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "worker {action} failed");
    }
    let app = body_json(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/orgs/default/apps/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(app["application"]["status"], "running");

    // Park the sleeper so no stray process survives the test run.
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/default/apps/{id}/stop"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn worker_settings_reject_web_only_fields() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/apps")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"name": "jobs", "binary_path": "/bin/sleep", "kind": "worker"})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let id = body_json(resp).await["application"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    for (uri, payload, field) in [
        (
            format!("/api/v1/orgs/default/apps/{id}"),
            json!({"domain": "jobs.example.test"}),
            "domain",
        ),
        (
            format!("/api/v1/orgs/default/apps/{id}"),
            json!({"metrics_path": "/metrics"}),
            "metrics_path",
        ),
        (
            format!("/api/v1/orgs/default/apps/{id}/domains"),
            json!({"domain": "jobs.example.test"}),
            "domain",
        ),
    ] {
        let method = if uri.ends_with("/domains") {
            "POST"
        } else {
            "PATCH"
        };
        let resp = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body_json(resp).await["field"], field);
    }
}

#[tokio::test]
async fn command_app_rollback_returns_a_clear_field_error() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    let payload = json!({
        "name": "argvapp",
        "command": ["/usr/bin/true"],
        "port": 9943,
    });
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/apps")
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

    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/default/apps/{id}/rollback"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(resp).await;
    assert_eq!(body["field"], "command");
    assert!(body["detail"]
        .as_str()
        .unwrap()
        .contains("do not retain previous argv"));
}

#[tokio::test]
async fn static_rollback_rejects_unsupported_explicit_targets() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/apps")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "name": "staticrollback",
                        "kind": "static",
                        "publish_dir": "/tmp/not-deployed",
                        "port": 9944
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let id = body_json(resp).await["application"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/default/apps/{id}/rollback"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"artifact_hash": "dir:old"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body_json(resp).await["field"], "artifact_hash");
}

#[tokio::test]
async fn worker_deploys_without_health_gate() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let yaml = r#"
name: sleeper
kind: worker
command: [/bin/sleep, "60"]
"#;
    let report = apply_manifest(&state, "default", yaml, dir.path(), false)
        .await
        .unwrap();
    assert!(report.created);
    let app: turaes_core::models::Application =
        sqlx::query_as("SELECT * FROM applications WHERE name = 'sleeper'")
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(app.port, 0);

    // Deploy: no health gate, slot cut over on clean start.
    let (dep_id, out) = crate::routes::apps::deploy_app(&state, &app).await.unwrap();
    assert_eq!(out.state, turaes_runtime::RunState::Running);
    let active: Option<i64> =
        sqlx::query_scalar("SELECT active_port FROM applications WHERE name = 'sleeper'")
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert!(active.is_some());

    // Cleanup the stray sleeper.
    let app: turaes_core::models::Application =
        sqlx::query_as("SELECT * FROM applications WHERE name = 'sleeper'")
            .fetch_one(&state.pool)
            .await
            .unwrap();
    let rt = crate::routes::apps::runtime_for(&state.cfg, "proc");
    let spec = crate::routes::apps::active_spec(&state.cfg, &app);
    rt.stop(&spec).await.unwrap();
    let _ = dep_id;
}

#[tokio::test]
async fn command_service_deploys_and_serves() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    // A $PORT-honoring HTTP server as an argv command (no shell involved).
    let script = dir.path().join("srv.py");
    std::fs::write(
        &script,
        "import http.server, os\nclass H(http.server.BaseHTTPRequestHandler):\n def do_GET(self):\n  self.send_response(200);self.end_headers();self.wfile.write(b'ok')\n def log_message(self, *a): pass\nhttp.server.HTTPServer(('127.0.0.1', int(os.environ['PORT'])), H).serve_forever()\n",
    )
    .unwrap();
    let python = std::env::var("PYTHON").unwrap_or_else(|_| "python3".into());
    let yaml = format!(
        "name: pysvc\ncommand: [{python:?}, {}]\nport: 9950\n",
        script.to_string_lossy()
    );
    let report = apply_manifest(&state, "default", &yaml, dir.path(), false)
        .await
        .unwrap();
    assert!(report.created);

    let app: turaes_core::models::Application =
        sqlx::query_as("SELECT * FROM applications WHERE name = 'pysvc'")
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(app.binary_path, python);
    let (_dep_id, out) = crate::routes::apps::deploy_app(&state, &app).await.unwrap();
    assert_eq!(out.state, turaes_runtime::RunState::Running);

    // stop it again so the test process tree stays clean.
    let app: turaes_core::models::Application =
        sqlx::query_as("SELECT * FROM applications WHERE name = 'pysvc'")
            .fetch_one(&state.pool)
            .await
            .unwrap();
    let rt = crate::routes::apps::runtime_for(&state.cfg, "proc");
    let spec = crate::routes::apps::active_spec(&state.cfg, &app);
    rt.stop(&spec).await.unwrap();
}

#[tokio::test]
async fn static_apply_syncs_on_proc_apply() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let pubdir = dir.path().join("dist");
    std::fs::create_dir(&pubdir).unwrap();
    std::fs::write(pubdir.join("index.html"), "<h1>static</h1>").unwrap();
    let yaml = format!(
        "name: staticsite\nkind: static\nport: 9960\npublish_dir: {}\ndomain: static.example.com\n",
        pubdir.to_string_lossy()
    );
    let report = apply_manifest(&state, "default", &yaml, dir.path(), false)
        .await
        .unwrap();
    assert!(report.created);
    let app: turaes_core::models::Application =
        sqlx::query_as("SELECT * FROM applications WHERE name = 'staticsite'")
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(app.kind, "static");

    // Applying the spec syncs the slot public dir (no process spawned).
    let rt = crate::routes::apps::runtime_for(&state.cfg, "proc");
    let spec =
        crate::routes::apps::spec_for_slot(&state.cfg, &app, Some(turaes_runtime::Slot::A), 9960);
    rt.apply(&spec, &std::collections::BTreeMap::new())
        .await
        .unwrap();
    let served = std::fs::read_to_string(format!("{}/index.html", spec.public_dir())).unwrap();
    assert_eq!(served, "<h1>static</h1>");
}

#[tokio::test]
async fn static_rollback_needs_a_previous_slot() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let pubdir = dir.path().join("dist");
    std::fs::create_dir(&pubdir).unwrap();
    let yaml = format!(
        "name: rbstatic\nkind: static\nport: 9970\npublish_dir: {}\n",
        pubdir.to_string_lossy()
    );
    apply_manifest(&state, "default", &yaml, dir.path(), false)
        .await
        .unwrap();
    let app: turaes_core::models::Application =
        sqlx::query_as("SELECT * FROM applications WHERE name = 'rbstatic'")
            .fetch_one(&state.pool)
            .await
            .unwrap();

    // Never deployed: nothing to cut back to.
    let err = crate::routes::apps::rollback_static(&state, &app)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("never deployed"));
}

#[tokio::test]
async fn maintenance_toggle_parks_and_restores() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    let payload = json!({
        "name": "held",
        "binary_path": "/srv/held/app",
        "port": 8400,
        "domain": "held.test"
    });
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/apps")
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let created = body_json(resp).await;
    let id = created["application"]["id"].as_str().unwrap().to_string();
    assert_eq!(created["application"]["maintenance"], false);

    // Toggle on: parks the hostnames.
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/default/apps/{id}/maintenance"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"enabled":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_json(resp).await["maintenance"], true);

    // Persisted on the application.
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/orgs/default/apps/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_json(resp).await["application"]["maintenance"], true);

    // Toggle off: routes restore.
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/default/apps/{id}/maintenance"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"enabled":false}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_json(resp).await["maintenance"], false);
}

#[tokio::test]
async fn maintenance_toggle_parks_hostnames_in_router() {
    use turaes_proxy::ParkedReason;

    let dir = tempfile::tempdir().unwrap();
    let mut state = test_state(dir.path()).await;
    let router_handle = Arc::new(turaes_proxy::Router::new("localhost", Default::default()));
    state.proxy_router = Some(router_handle.clone());
    let router = app::build_router(state.clone());

    let payload = json!({
        "name": "held",
        "binary_path": "/srv/held/app",
        "port": 8400,
        "domain": "held.test"
    });
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/apps")
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let created = body_json(resp).await;
    let id = created["application"]["id"].as_str().unwrap().to_string();

    // Fresh (never deployed, status stopped) parks as Stopped, not routed.
    crate::routes::apps::refresh_proxy_routes(&state)
        .await
        .unwrap();
    let parked = router_handle.is_parked("held.test").expect("host parked");
    assert_eq!(parked.app, "held");
    assert_eq!(parked.reason, ParkedReason::Stopped);
    assert!(router_handle.resolve("held.test").is_none());

    // Toggle on via the API: the handler itself republishes as Maintenance.
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/default/apps/{id}/maintenance"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"enabled":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let parked = router_handle.is_parked("held.test").expect("host parked");
    assert_eq!(parked.reason, ParkedReason::Maintenance);

    // Toggle off falls back to the status-driven reason.
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/default/apps/{id}/maintenance"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"enabled":false}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let parked = router_handle.is_parked("held.test").expect("host parked");
    assert_eq!(parked.reason, ParkedReason::Stopped);

    // A running app restores routes.
    sqlx::query("UPDATE applications SET status = 'running' WHERE id = ?")
        .bind(&id)
        .execute(&state.pool)
        .await
        .unwrap();
    crate::routes::apps::refresh_proxy_routes(&state)
        .await
        .unwrap();
    assert!(router_handle.is_parked("held.test").is_none());
    assert_eq!(router_handle.resolve("held.test").unwrap().port, 8400);
}

// --- Registry -----------------------------------------------------------

/// Minimal ELF bytes with the given machine id (uploads require ELF).
fn elf_bytes(machine: u16) -> Vec<u8> {
    let mut b = vec![0u8; 64];
    b[0..4].copy_from_slice(b"\x7fELF");
    b[4] = 2;
    b[5] = 1;
    b[6] = 1;
    b[16..18].copy_from_slice(&2u16.to_le_bytes());
    b[18..20].copy_from_slice(&machine.to_le_bytes());
    b
}

/// gzip tarball with a top-level manifest.json plus `files/` members.
fn bundle_bytes(manifest: &str, files: &[(&str, Vec<u8>)]) -> Vec<u8> {
    use std::io::Write;
    let mut tar_data = Vec::new();
    {
        let mut tar = tar::Builder::new(&mut tar_data);
        let mut header = tar::Header::new_gnu();
        header.set_size(manifest.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, "manifest.json", manifest.as_bytes())
            .unwrap();
        for (name, bytes) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            tar.append_data(&mut header, format!("files/{name}"), &bytes[..])
                .unwrap();
        }
        tar.finish().unwrap();
    }
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(&tar_data).unwrap();
    enc.finish().unwrap()
}

/// Remote app (agent-reconciled, so deploys queue without executing) with a
/// linked CI repo. Returns the app id.
async fn registry_app(state: &AppState, router: &axum::Router, name: &str) -> String {
    sqlx::query(
        "INSERT OR IGNORE INTO servers (id, name, address) VALUES ('edge1', 'edge1', '10.0.0.1')",
    )
    .execute(&state.pool)
    .await
    .unwrap();
    let payload = json!({
        "name": name,
        "binary_path": format!("/srv/{name}/bin"),
        "port": 19000,
        "server_id": "edge1",
    });
    let created = body_json(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/default/apps")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let id = created["application"]["id"].as_str().unwrap().to_string();
    let link = json!({"application_id": id, "repo": "acme/regapp"});
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/registry/links")
                .header("content-type", "application/json")
                .body(Body::from(link.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    id
}

fn upload_uri(app: &str, query: &str) -> String {
    format!("/api/v1/orgs/default/registry/artifacts?app={app}&repo=acme%2Fregapp{query}")
}

#[tokio::test]
async fn registry_push_binary_creates_release_without_channel() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let router = app::build_router(state.clone());
    let id = registry_app(&state, &router, "regpush").await;

    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(upload_uri("regpush", "&version=1.0.0&commit=abc123"))
                .body(Body::from(elf_bytes(62)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = body_json(resp).await;
    assert_eq!(body["release"]["version"], "1.0.0");
    assert!(body["release"].get("channel").is_none());
    assert!(body["artifact"]["hash"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    assert_eq!(body["artifact"]["arch"], "x86_64");
    assert!(body["ignored_hints"].as_array().unwrap().is_empty());
    let rel_id = body["release"]["id"].as_str().unwrap().to_string();

    // No channel moves on push: latest does not exist yet.
    let listed = body_json(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/orgs/default/registry/{id}/releases"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(listed["releases"].as_array().unwrap().len(), 1);
    assert!(listed["channels"].as_array().unwrap().is_empty());
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/orgs/default/registry/{id}/resolve?channel=latest"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Explicit promote moves latest; resolve pins the same hash.
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/default/registry/releases/{rel_id}/promote"
                ))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"channel":"latest"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let listed = body_json(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/orgs/default/registry/{id}/releases"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(listed["channels"][0]["channel"], "latest");
    assert_eq!(listed["channels"][0]["version"], "1.0.0");
    let resolved = body_json(
        router
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/orgs/default/registry/{id}/resolve?channel=latest"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(resolved["hash"], body["artifact"]["hash"]);
}

#[tokio::test]
async fn registry_deploy_by_version_queues_remote_hash() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let router = app::build_router(state.clone());
    let id = registry_app(&state, &router, "regdeploy").await;
    let pushed = body_json(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(upload_uri("regdeploy", "&version=2.1.0"))
                    .body(Body::from(elf_bytes(183)))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let hash = pushed["artifact"]["hash"].as_str().unwrap().to_string();

    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/default/apps/{id}/deploy"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"version":"2.1.0"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let out = body_json(resp).await;
    assert_eq!(out["version"], "2.1.0");
    assert_eq!(out["artifact_hash"], hash);
    // Remote path queues without executing: deployment row pins the hash.
    let row: Option<String> = sqlx::query_scalar(
        "SELECT artifact_hash FROM deployments WHERE application_id = ? ORDER BY rowid DESC LIMIT 1",
    )
    .bind(&id)
    .fetch_optional(&state.pool)
    .await
    .unwrap()
    .flatten();
    assert_eq!(row, Some(hash));

    // Unknown versions and yanked channels never deploy.
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/default/apps/{id}/deploy"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"version":"9.9.9"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/default/apps/{id}/deploy"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"version":"2.1.0","channel":"stable"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn registry_push_rejects_unlinked_repo_and_bad_input() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let router = app::build_router(state.clone());
    registry_app(&state, &router, "regreject").await;

    // Unlinked repo: forbidden with a linking hint.
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/registry/artifacts?app=regreject&repo=acme%2Fstranger&version=1.0.0")
                .body(Body::from(elf_bytes(62)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Bad semver names the field.
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(upload_uri("regreject", "&version=latest"))
                .body(Body::from(elf_bytes(62)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(resp).await;
    assert_eq!(body["field"], "version");

    // Non-binaries are refused.
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(upload_uri("regreject", "&version=1.0.0"))
                .body(Body::from("#!/bin/sh\necho hi\n"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Runtime hints are reported, never applied.
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(upload_uri(
                    "regreject",
                    "&version=1.0.1&port=9999&domain=evil.test",
                ))
                .body(Body::from(elf_bytes(62)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = body_json(resp).await;
    let hints: Vec<&str> = body["ignored_hints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(hints.contains(&"port") && hints.contains(&"domain"));

    // Duplicate versions conflict.
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(upload_uri("regreject", "&version=1.0.1"))
                .body(Body::from(elf_bytes(62)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn registry_bundle_push_validates_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let router = app::build_router(state.clone());
    let id = registry_app(&state, &router, "regbundle").await;

    let manifest = r#"{"name":"regbundle","version":"3.0.0","arch":"x86_64","commit":"def456","files":[{"path":"app"},{"path":"helper"}]}"#;
    let bundle = bundle_bytes(
        manifest,
        &[("app", elf_bytes(62)), ("helper", elf_bytes(62))],
    );
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(upload_uri("regbundle", "&version=3.0.0"))
                .body(Body::from(bundle))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = body_json(resp).await;
    assert_eq!(body["release"]["version"], "3.0.0");
    assert_eq!(body["release"]["files"].as_array().unwrap().len(), 2);
    let resolved = body_json(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/orgs/default/registry/{id}/resolve?version=3.0.0"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(resolved["hash"], body["artifact"]["hash"]);

    // Unknown manifest keys are rejected.
    let bad = bundle_bytes(
        r#"{"name":"regbundle","port":9999}"#,
        &[("app", elf_bytes(62))],
    );
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(upload_uri("regbundle", ""))
                .body(Body::from(bad))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Query/manifest version conflicts fail instead of merging silently.
    let clash = bundle_bytes(
        r#"{"name":"regbundle","version":"3.0.1"}"#,
        &[("app", elf_bytes(62))],
    );
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(upload_uri("regbundle", "&version=3.0.2"))
                .body(Body::from(clash))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn registry_promote_and_yank_move_channels() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let router = app::build_router(state.clone());
    let id = registry_app(&state, &router, "regchan").await;

    // Artifact-only push, then pin it via the releases endpoint.
    let pushed = body_json(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(upload_uri("regchan", ""))
                    .body(Body::from(elf_bytes(62)))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(pushed.get("release").unwrap().is_null());
    let hash = pushed["artifact"]["hash"].as_str().unwrap().to_string();
    let rel = body_json(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/default/registry/releases")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"application_id": id, "version": "4.0.0", "artifact_hash": hash})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let rel_id = rel["release"]["id"].as_str().unwrap().to_string();

    // Promote to stable (dev resolves to owner in tests).
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/default/registry/releases/{rel_id}/promote"
                ))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"channel":"stable"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resolved = body_json(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/orgs/default/registry/{id}/resolve?channel=stable"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(resolved["version"], "4.0.0");

    // Yank hides the release from every channel and from resolve.
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/default/registry/releases/{rel_id}/yank"
                ))
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
                .uri(format!(
                    "/api/v1/orgs/default/registry/{id}/resolve?channel=stable"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let resp = router
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/orgs/default/registry/{id}/resolve?version=4.0.0"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn registry_links_crud_and_scoping() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let router = app::build_router(state.clone());
    let id = registry_app(&state, &router, "reglinks").await;

    // Duplicate links conflict.
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/registry/links")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"application_id": id, "repo": "acme/regapp"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // Bad repo shapes name the field.
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/registry/links")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"application_id": id, "repo": "not-a-repo"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Unlink, then unlinking again is 404.
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!(
                    "/api/v1/orgs/default/registry/links?application_id={id}&repo=acme%2Fregapp"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!(
                    "/api/v1/orgs/default/registry/links?application_id={id}&repo=acme%2Fregapp"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Another org cannot see this org's registry surface (unknown, not forbidden).
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ('other', 'other', 'Other')")
        .execute(&state.pool)
        .await
        .unwrap();
    let resp = router
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/orgs/other/registry/{id}/releases"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn catalog_returns_seeded_templates_and_platform() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    let resp = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/orgs/default/catalog")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let templates = body["templates"].as_array().unwrap();
    assert_eq!(templates.len(), 3);
    let slugs: Vec<&str> = templates
        .iter()
        .map(|t| t["slug"].as_str().unwrap())
        .collect();
    assert_eq!(slugs, vec!["axum-service", "static-site", "worker"]);
    for t in templates {
        assert!(!t["name"].as_str().unwrap().is_empty());
        assert!(t["defaults"].is_object());
    }
    assert!(!body["platform"]["version"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn catalog_is_org_scoped() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let router = app::build_router(state.clone());
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ('other', 'other', 'Other')")
        .execute(&state.pool)
        .await
        .unwrap();

    // An org the caller is not a member of is indistinguishable from
    // an unknown org...
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/orgs/other/catalog")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // ...while an unknown org is not found.
    let resp = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/orgs/nope/catalog")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn registry_write_endpoints_require_admin() {
    // A deploy-scoped token (Developer floor) must not push executables,
    // pin releases, or manage repo links — app creation is Admin-only.
    let dir = tempfile::tempdir().unwrap();
    let (_, deploy_token) = mint_token(test_router(dir.path()).await, "deploy").await;
    let router = test_router(dir.path()).await;
    let authed = |method: &str, uri: String, body: Body| {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", format!("Bearer {deploy_token}"))
            .header("content-type", "application/json")
            .body(body)
            .unwrap()
    };

    let resp = router
        .clone()
        .oneshot(authed(
            "POST",
            "/api/v1/orgs/default/registry/artifacts?app=nope".into(),
            Body::from(vec![0x7f, b'E', b'L', b'F', 0, 1]),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let resp = router
        .clone()
        .oneshot(authed(
            "POST",
            "/api/v1/orgs/default/registry/releases".into(),
            Body::from(
                json!({"application_id": "nope", "version": "1.0.0", "artifact_hash": "sha256:00"})
                    .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let resp = router
        .clone()
        .oneshot(authed(
            "POST",
            "/api/v1/orgs/default/registry/links".into(),
            Body::from(json!({"application_id": "nope", "repo": "acme/app"}).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let resp = router
        .oneshot(authed(
            "DELETE",
            "/api/v1/orgs/default/registry/links?application_id=nope&repo=acme%2Fapp".into(),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn registry_bundle_push_enforces_hardening_rules() {
    use std::io::Write;

    fn gzip_tar(entries: Vec<(tar::Header, &str, Vec<u8>)>) -> Vec<u8> {
        let mut tar_data = Vec::new();
        {
            let mut tar = tar::Builder::new(&mut tar_data);
            for (mut header, path, bytes) in entries {
                header.set_size(bytes.len() as u64);
                header.set_cksum();
                tar.append_data(&mut header, path, &bytes[..]).unwrap();
            }
            tar.finish().unwrap();
        }
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&tar_data).unwrap();
        enc.finish().unwrap()
    }

    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let router = app::build_router(state.clone());
    let _id = registry_app(&state, &router, "regharden").await;
    let push = |uri: String, body: Vec<u8>| {
        let router = router.clone();
        async move {
            router
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(uri)
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap()
        }
    };
    let tmp_count = || {
        std::fs::read_dir(dir.path().join("artifacts").join("sha256"))
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp-"))
                    .count()
            })
            .unwrap_or(0)
    };
    let manifest =
        |v: &str| format!(r#"{{"name":"regharden","version":"{v}","files":[{{"path":"app"}}]}}"#);

    // Symlink members are rejected and leave no temp files.
    let mut link_header = tar::Header::new_gnu();
    let bundle = gzip_tar(vec![
        (
            {
                let mut h = tar::Header::new_gnu();
                h.set_size(manifest("9.0.0").len() as u64);
                h.set_cksum();
                h
            },
            "manifest.json",
            manifest("9.0.0").into_bytes(),
        ),
        (
            {
                link_header.set_size(0);
                link_header.set_entry_type(tar::EntryType::Symlink);
                link_header.set_link_name("target").unwrap();
                link_header.set_cksum();
                link_header
            },
            "files/evil",
            vec![],
        ),
    ]);
    let resp = push(upload_uri("regharden", "&version=9.0.0"), bundle).await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(tmp_count(), 0);

    // Non-string identity fields are rejected, not silently downgraded.
    let bundle = bundle_bytes(
        r#"{"name":"regharden","version":123,"files":[{"path":"app"}]}"#,
        &[("app", elf_bytes(62))],
    );
    let resp = push(upload_uri("regharden", "&version=9.0.1"), bundle).await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(tmp_count(), 0);

    // Undeclared files/ extras are rejected.
    let bundle = bundle_bytes(
        &manifest("9.0.2"),
        &[("app", elf_bytes(62)), ("extra", elf_bytes(62))],
    );
    let resp = push(upload_uri("regharden", "&version=9.0.2"), bundle).await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(tmp_count(), 0);

    // Unknown query fields fail loudly.
    let resp = push(
        upload_uri("regharden", "&version=9.0.3&versoin=1"),
        elf_bytes(62),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // commit_sha is accepted as the canonical commit field.
    let resp = push(
        upload_uri("regharden", "&version=9.0.4&commit_sha=abc123"),
        elf_bytes(62),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Pinned digest mismatch is rejected; match succeeds.
    use sha2::Digest;
    let raw = elf_bytes(62);
    let hex = format!("{:x}", sha2::Sha256::digest(&raw));
    let resp = push(
        upload_uri("regharden", "&version=9.0.5&sha256=sha256:00"),
        raw.clone(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let resp = push(
        upload_uri("regharden", &format!("&version=9.0.6&sha256={hex}")),
        raw,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(tmp_count(), 0);
}

#[tokio::test]
async fn registry_create_release_rejects_foreign_hash() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let router = app::build_router(state.clone());
    // Victim org (default): app + pushed bytes.
    let _victim = registry_app(&state, &router, "victim").await;
    let pushed = body_json(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(upload_uri("victim", "&version=1.0.0"))
                    .body(Body::from(elf_bytes(62)))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let victim_hash = pushed["artifact"]["hash"].as_str().unwrap().to_string();

    // Attacker org with its own app + linked repo; dev user joined as owner.
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ('other', 'other', 'Other')")
        .execute(&state.pool)
        .await
        .unwrap();
    let dev_id: String = sqlx::query_scalar("SELECT id FROM users WHERE github_id = 0")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO memberships (id, org_id, user_id, role) VALUES ('m9', 'other', ?, 'owner')",
    )
    .bind(&dev_id)
    .execute(&state.pool)
    .await
    .unwrap();
    let payload = json!({"name": "attacker", "binary_path": "/srv/attacker/bin", "port": 19100});
    let created = body_json(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/other/apps")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let attacker_id = created["application"]["id"].as_str().unwrap().to_string();
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/other/registry/links")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"application_id": attacker_id, "repo": "acme/other"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Pinning the victim's hash into the attacker org is rejected...
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/other/registry/releases")
                .header("content-type", "application/json")
                .body(
                    Body::from(
                        json!({"application_id": attacker_id, "version": "1.0.0", "artifact_hash": victim_hash})
                            .to_string(),
                    ),
                )
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // ...while the org's own pushed bytes pin fine.
    let own = body_json(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/other/registry/artifacts?app=attacker&repo=acme%2Fother&version=2.0.0")
                    .body(Body::from(elf_bytes(183)))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let own_hash = own["artifact"]["hash"].as_str().unwrap().to_string();
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/other/registry/releases")
                .header("content-type", "application/json")
                .body(
                    Body::from(
                        json!({"application_id": attacker_id, "version": "1.0.0", "artifact_hash": own_hash})
                            .to_string(),
                    ),
                )
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn registry_agent_download_scoped_to_placement() {
    use crate::grpc::pb::RegisterRequest;

    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let router = app::build_router(state.clone());
    let reg_w1 = crate::grpc::register(
        &state,
        RegisterRequest {
            join_token: "test-join".into(),
            name: "w1".into(),
            address: "10.0.0.9".into(),
            version: "0".into(),
        },
    )
    .await
    .unwrap();
    let reg_w2 = crate::grpc::register(
        &state,
        RegisterRequest {
            join_token: "test-join".into(),
            name: "w2".into(),
            address: "10.0.0.10".into(),
            version: "0".into(),
        },
    )
    .await
    .unwrap();

    // App placed on w1 with a pushed release.
    let payload = json!({"name": "nodeapp", "binary_path": "/srv/nodeapp/bin", "port": 19200, "server_id": reg_w1.server_id});
    let created = body_json(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/default/apps")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let app_id = created["application"]["id"].as_str().unwrap().to_string();
    let link = json!({"application_id": app_id, "repo": "acme/nodeapp"});
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/registry/links")
                .header("content-type", "application/json")
                .body(Body::from(link.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let pushed = body_json(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/default/registry/artifacts?app=nodeapp&repo=acme%2Fnodeapp&version=1.0.0")
                    .body(Body::from(elf_bytes(62)))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let hash = pushed["artifact"]["hash"].as_str().unwrap().to_string();

    // Placed node's agent fetches fine...
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/agent/artifacts/{hash}"))
                .header("authorization", format!("Bearer {}", reg_w1.agent_token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // ...an unplaced node's agent cannot, same 404 as missing.
    let resp = router
        .oneshot(
            Request::builder()
                .uri(format!("/agent/artifacts/{hash}"))
                .header("authorization", format!("Bearer {}", reg_w2.agent_token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn registry_provenance_caps_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let router = app::build_router(state.clone());
    let app_id = registry_app(&state, &router, "regprov").await;

    // Oversized notes, bogus arch, non-http build URL all fail.
    let big = "n".repeat(5000);
    for query in [
        format!("&version=5.0.0&notes={big}"),
        "&version=5.0.1&arch=bogus".to_string(),
        "&version=5.0.2&build_url=ftp://example.com/x".to_string(),
    ] {
        let resp = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(upload_uri("regprov", &query))
                    .body(Body::from(elf_bytes(62)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
    // create_release inputs capped too.
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/registry/releases")
                .header("content-type", "application/json")
                .body(
                    Body::from(
                        json!({"application_id": app_id, "version": "1.0.0", "artifact_hash": "sha256:00", "notes": big})
                            .to_string(),
                    ),
                )
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn registry_rollback_refuses_yanked_without_force_admin() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let router = app::build_router(state.clone());
    // Local app: rollback executes here.
    let payload = json!({"name": "localrb", "binary_path": "/srv/localrb/bin", "port": 19300});
    let created = body_json(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/default/apps")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let app_id = created["application"]["id"].as_str().unwrap().to_string();
    let link = json!({"application_id": app_id, "repo": "acme/localrb"});
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/registry/links")
                .header("content-type", "application/json")
                .body(Body::from(link.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let pushed = body_json(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/default/registry/artifacts?app=localrb&repo=acme%2Flocalrb&version=1.0.0")
                    .body(Body::from(elf_bytes(62)))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let hash = pushed["artifact"]["hash"].as_str().unwrap().to_string();
    let rel_id = pushed["release"]["id"].as_str().unwrap().to_string();
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/orgs/default/registry/releases/{rel_id}/yank"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    // The yanked build ran here before (deployment history is the
    // ownership proof rollback demands).
    sqlx::query(
        "INSERT INTO deployments (id, application_id, status, artifact_hash, started_at) \
         VALUES ('d-rb', (SELECT id FROM applications WHERE name = 'localrb'), 'running', ?, datetime('now'))",
    )
    .bind(&hash)
    .execute(&state.pool)
    .await
    .unwrap();

    // Yanked hash cannot come back by default...
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/default/apps/{app_id}/rollback"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"artifact_hash": hash}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // ...nor with force on a deploy-scoped (Developer-floor) token.
    let (_, deploy_token) = mint_token(test_router(dir.path()).await, "deploy").await;
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/default/apps/{app_id}/rollback"))
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {deploy_token}"))
                .body(Body::from(
                    json!({"artifact_hash": hash, "force": true}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn registry_version_deploy_records_provenance() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let router = app::build_router(state.clone());
    let app_id = registry_app(&state, &router, "regprov2").await;
    let pushed = body_json(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(upload_uri("regprov2", "&version=7.1.0"))
                    .body(Body::from(elf_bytes(62)))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let rel_id = pushed["release"]["id"].as_str().unwrap().to_string();
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/default/apps/{app_id}/deploy"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"version":"7.1.0"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let row: (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT release_id, resolved_version FROM deployments WHERE application_id = ? ORDER BY rowid DESC LIMIT 1",
    )
    .bind(&app_id)
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert_eq!(row, (Some(rel_id), Some("7.1.0".to_string())));
}

#[tokio::test]
async fn registry_gc_expires_orphans_and_delete_endpoint_guards_pins() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let router = app::build_router(state.clone());
    let app_id = registry_app(&state, &router, "reggc").await;

    // Pinned blob: push + release, then backdate both row and file.
    let pushed = body_json(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(upload_uri("reggc", "&version=1.0.0"))
                    .body(Body::from(elf_bytes(62)))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let pinned_hash: String = pushed["artifact"]["hash"].as_str().unwrap().to_string();
    // Orphan blob: pushed with no version (no release row).
    let orphan = body_json(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(upload_uri("reggc", ""))
                    .body(Body::from(elf_bytes(183)))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let orphan_hash: String = orphan["artifact"]["hash"].as_str().unwrap().to_string();

    sqlx::query("UPDATE artifacts SET created_at = datetime('now', '-31 days')")
        .execute(&state.pool)
        .await
        .unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(31 * 86400);
    for h in [&pinned_hash, &orphan_hash] {
        let bare = h.strip_prefix("sha256:").unwrap_or(h);
        let p = dir.path().join("artifacts").join("sha256").join(bare);
        std::fs::File::options()
            .write(true)
            .open(&p)
            .unwrap()
            .set_modified(old)
            .unwrap();
    }

    let report = crate::gc::collect_artifacts(&state, false).await.unwrap();
    assert_eq!(report.rows_expired, 1);
    // Orphan blob + row gone; pinned blob + row survive.
    let bare_orphan = orphan_hash.strip_prefix("sha256:").unwrap_or(&orphan_hash);
    assert!(!dir
        .path()
        .join("artifacts")
        .join("sha256")
        .join(bare_orphan)
        .exists());
    let gone: i64 = sqlx::query_scalar("SELECT count(*) FROM artifacts WHERE hash = ?")
        .bind(&orphan_hash)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(gone, 0);
    let bare_pinned = pinned_hash.strip_prefix("sha256:").unwrap_or(&pinned_hash);
    assert!(dir
        .path()
        .join("artifacts")
        .join("sha256")
        .join(bare_pinned)
        .exists());

    // DELETE refuses the pinned blob...
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!(
                    "/api/v1/orgs/default/registry/artifacts/{pinned_hash}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // ...and 404s a blob that was never stored.
    let resp = router
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/orgs/default/registry/artifacts/sha256:00")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let _ = app_id;
}

#[tokio::test]
async fn deploy_locks_serialize_per_app() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let (tx, mut rx) = tokio::sync::mpsc::channel::<&str>(4);
    // First holder takes the app-1 lock, then signals it holds it.
    let s1 = state.clone();
    let tx1 = tx.clone();
    let t1 = tokio::spawn(async move {
        let _guard = s1.deploy_lock("app-1").await;
        tx1.send("t1-in").await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        tx1.send("t1-out").await.unwrap();
    });
    assert_eq!(rx.recv().await.unwrap(), "t1-in");
    // A contender for the same app, plus an unrelated app that must proceed.
    let s2 = state.clone();
    let tx2 = tx.clone();
    let t2 = tokio::spawn(async move {
        let _guard = s2.deploy_lock("app-1").await;
        tx2.send("t2-in").await.unwrap();
    });
    let s3 = state.clone();
    let t3 = tokio::spawn(async move {
        let _guard = s3.deploy_lock("app-2").await;
    });
    t1.await.unwrap();
    t2.await.unwrap();
    t3.await.unwrap();
    drop(tx);
    let mut order = Vec::new();
    while let Some(m) = rx.recv().await {
        order.push(m);
    }
    // t2 entered only after t1 released: same-app deploys serialize.
    assert_eq!(order, vec!["t1-out", "t2-in"]);
}

#[tokio::test]
async fn monitor_set_status_never_leaves_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    sqlx::query(
        "INSERT INTO applications (id, name, binary_path, port, server_id, org_id, status) \
         VALUES ('a1', 'stopped-app', '/bin/true', 19999, 'local', 'default', 'stopped')",
    )
    .execute(&state.pool)
    .await
    .unwrap();
    crate::monitor::set_status(&state, "a1", "running")
        .await
        .unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM applications WHERE id = 'a1'")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(status, "stopped");

    sqlx::query("UPDATE applications SET status = 'running' WHERE id = 'a1'")
        .execute(&state.pool)
        .await
        .unwrap();
    crate::monitor::set_status(&state, "a1", "unhealthy")
        .await
        .unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM applications WHERE id = 'a1'")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(status, "unhealthy");
}

#[tokio::test]
async fn app_create_parks_hostname_until_first_deploy() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = test_state(dir.path()).await;
    let router_handle = Arc::new(turaes_proxy::Router::new("localhost", Default::default()));
    state.proxy_router = Some(router_handle.clone());
    let router = app::build_router(state.clone());

    let payload = json!({
        "name": "fresh",
        "binary_path": "/srv/fresh/bin",
        "port": 19800,
        "domain": "fresh.test"
    });
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/apps")
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    // The create handler republishes: never-deployed hostname parks as
    // Stopped instead of 404ing.
    let parked = router_handle.is_parked("fresh.test").expect("host parked");
    assert_eq!(parked.app, "fresh");
    assert!(router_handle.resolve("fresh.test").is_none());
}

#[tokio::test]
async fn agent_report_republishes_parked_routes() {
    use crate::grpc::pb::RegisterRequest;

    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let reg = crate::grpc::register(
        &state,
        RegisterRequest {
            join_token: "test-join".into(),
            name: "w9".into(),
            address: "10.0.0.19".into(),
            version: "0".into(),
        },
    )
    .await
    .unwrap();
    let mut state = state;
    let router_handle = Arc::new(turaes_proxy::Router::new("localhost", Default::default()));
    state.proxy_router = Some(router_handle.clone());
    let router = app::build_router(state.clone());

    let payload = json!({
        "name": "nodeapp",
        "binary_path": "/srv/nodeapp/bin",
        "port": 19700,
        "domain": "nodeapp.test",
        "server_id": reg.server_id,
    });
    let created = body_json(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/default/apps")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let app_name = created["application"]["name"].as_str().unwrap().to_string();

    // Agent reports stopped: routes republish with the host parked.
    crate::grpc::report(
        &state,
        crate::grpc::pb::ReportRequest {
            agent_token: reg.agent_token.clone(),
            app_name: app_name.clone(),
            status: "stopped".into(),
            message: "halted".into(),
            artifact_hash: String::new(),
        },
    )
    .await
    .unwrap();
    let parked = router_handle
        .is_parked("nodeapp.test")
        .expect("host parked");
    assert_eq!(parked.app, app_name);

    // Agent reports running: routes restore.
    crate::grpc::report(
        &state,
        crate::grpc::pb::ReportRequest {
            agent_token: reg.agent_token.clone(),
            app_name,
            status: "running".into(),
            message: "back".into(),
            artifact_hash: String::new(),
        },
    )
    .await
    .unwrap();
    assert!(router_handle.is_parked("nodeapp.test").is_none());
}

#[tokio::test]
async fn quota_gate_serializes_concurrent_creates() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let router = app::build_router(state.clone());
    sqlx::query("INSERT OR IGNORE INTO org_quotas (org_id) VALUES ('default')")
        .execute(&state.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE org_quotas SET max_apps = 1 WHERE org_id = 'default'")
        .execute(&state.pool)
        .await
        .unwrap();

    let mk = |name: &str, port: u16| {
        let router = router.clone();
        let payload = json!({"name": name, "binary_path": "/srv/x/bin", "port": port}).to_string();
        async move {
            router
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/v1/orgs/default/apps")
                        .header("content-type", "application/json")
                        .body(Body::from(payload))
                        .unwrap(),
                )
                .await
                .unwrap()
                .status()
        }
    };
    let (a, b) = tokio::join!(mk("quota-a", 21101), mk("quota-b", 21102));
    let mut statuses = vec![a, b];
    statuses.sort();
    // Exactly one admits; the loser sees the quota, never two apps.
    assert_eq!(statuses, vec![StatusCode::CREATED, StatusCode::CONFLICT]);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM applications WHERE org_id = 'default'")
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn port_pair_range_and_zero_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    // Slot B overflows u16 (test slot offset is 1000).
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/orgs/default/apps")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"name": "bigport", "binary_path": "/srv/x/bin", "port": 65000})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let created = body_json(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/default/apps")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"name": "porty", "binary_path": "/srv/x/bin", "port": 21200})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let id = created["application"]["id"].as_str().unwrap().to_string();
    // Zeroing a service port and overflowing the pair both fail on update.
    for payload in [json!({"port": 0}), json!({"port": 65000})] {
        let resp = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/v1/orgs/default/apps/{id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}

#[tokio::test]
async fn placement_change_resets_slot_and_asks_redeploy() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(dir.path()).await;
    let created = body_json(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/default/apps")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"name": "mover", "binary_path": "/srv/x/bin", "port": 21300})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let id = created["application"]["id"].as_str().unwrap().to_string();
    let resp = router
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/orgs/default/apps/{id}"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"port": 21400}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert!(body["application"]["active_port"].is_null());
    assert_eq!(body["application"]["port"], 21400);
    assert!(body["note"].as_str().unwrap().contains("redeploy"));
}

#[tokio::test]
async fn rollback_rejects_foreign_hash() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let router = app::build_router(state.clone());
    for (name, port) in [("owner-a", 21501), ("owner-b", 21502)] {
        let payload = json!({"name": name, "binary_path": "/srv/x/bin", "port": port});
        let resp = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/orgs/default/apps")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }
    // B ran hash H (deployment row + blob in store); A never did.
    let hash = format!("sha256:{}", "ab".repeat(32));
    sqlx::query(
        "INSERT INTO deployments (id, application_id, status, artifact_hash, started_at) \
         VALUES ('d-b', (SELECT id FROM applications WHERE name = 'owner-b'), 'running', ?, datetime('now'))",
    )
    .bind(&hash)
    .execute(&state.pool)
    .await
    .unwrap();
    let bare = "ab".repeat(32);
    std::fs::create_dir_all(dir.path().join("artifacts").join("sha256")).unwrap();
    std::fs::write(
        dir.path().join("artifacts").join("sha256").join(&bare),
        b"fake-bytes",
    )
    .unwrap();
    let app_a: String = sqlx::query_scalar("SELECT id FROM applications WHERE name = 'owner-a'")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/orgs/default/apps/{app_a}/rollback"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"artifact_hash": hash}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = body_json(resp).await;
    assert!(body["detail"].as_str().unwrap().contains("never ran under"));
}

#[tokio::test]
async fn quota_put_and_validate_are_audited() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let router = app::build_router(state.clone());

    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/orgs/default/quota")
                .header("content-type", "application/json")
                .body(Body::from(json!({"max_apps": 7}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let action: Option<String> =
        sqlx::query_scalar("SELECT action FROM audit_log WHERE action = 'quota.set'")
            .fetch_optional(&state.pool)
            .await
            .unwrap();
    assert_eq!(action.as_deref(), Some("quota.set"));

    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/servers/local/validate")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let action: Option<String> =
        sqlx::query_scalar("SELECT action FROM audit_log WHERE action = 'server.validate'")
            .fetch_optional(&state.pool)
            .await
            .unwrap();
    assert_eq!(action.as_deref(), Some("server.validate"));
}
