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
    let bin = write_health_server(dir.path());
    let id = create_app(&router, "dapp", &bin, 9201).await;

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
    let bin = write_health_server(dir.path());
    let id = create_app(&router, "happ", &bin, 9600).await;

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
                .uri(format!("/api/v1/apps/{id}/env/1BAD"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"value": "x"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // set
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/apps/{id}/env/API_KEY"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"value": "secret123"}).to_string()))
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
                .uri(format!("/api/v1/apps/{id}/env"))
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
                .uri(format!("/api/v1/apps/{id}/env/API_KEY"))
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
                .uri("/api/v1/apps")
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
                .uri("/api/v1/apps")
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
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/apps/{id}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"port": 9801, "domain": "u.test"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["application"]["port"], 9801);
    assert_eq!(body["application"]["domain"], "u.test");
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
                .uri(format!("/api/v1/apps/{id}/stop"))
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
                .uri(format!("/api/v1/apps/{id}/deploy"))
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
                .uri(format!("/api/v1/apps/{id}/rollback"))
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
                .uri(format!("/api/v1/apps/{id}/domains"))
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
                .uri(format!("/api/v1/apps/{id}/domains"))
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
                .uri(format!("/api/v1/apps/{id}/domains"))
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
                .uri(format!("/api/v1/apps/{id}/domains/www.example.com"))
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
                .uri(format!("/api/v1/apps/{id}/deploy"))
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
                    .uri(format!("/api/v1/apps/{id}"))
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
                    .uri(format!("/api/v1/apps/{id}"))
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
                .uri(format!("/api/v1/apps/{id}/deploy"))
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
                    .uri(format!("/api/v1/apps/{id}"))
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
