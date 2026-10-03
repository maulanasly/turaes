//! `turaes agent` — reconcile desired state from the control plane.
//!
//! Loop: heartbeat → poll desired apps → fetch missing artifacts over HTTP →
//! install/restart via the local runtime → report status. Outbound only.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use turaes_core::artifact::ArtifactStore;
use turaes_core::config::Config;
use turaes_core::Result;
use turaes_runtime::{AppSpec, Deployer};

use crate::grpc::pb;
use crate::routes::apps::runtime_for;

/// Arguments for the agent role.
#[derive(Debug, Clone)]
pub struct AgentArgs {
    /// Control-plane gRPC endpoint, e.g. `http://10.0.0.2:9443`.
    pub control: String,
    /// Control-plane HTTP base for artifact downloads, e.g. `http://10.0.0.2:8787`.
    pub http: String,
    /// Shared join token.
    pub token: String,
    /// Node name (unique in the fleet).
    pub name: String,
    /// Reachable address advertised to the control plane.
    pub address: String,
    /// Heartbeat/poll interval, seconds.
    pub interval: u64,
}

/// Register, then reconcile desired state forever.
pub async fn run(cfg: Arc<Config>, args: AgentArgs) -> Result<()> {
    let mut client = pb::control_client::ControlClient::connect(args.control.clone())
        .await
        .map_err(|e| {
            turaes_core::Error::Internal(format!("failed to connect to {}: {e}", args.control))
        })?;

    let registered = client
        .register(pb::RegisterRequest {
            join_token: args.token.clone(),
            name: args.name.clone(),
            address: args.address.clone(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        })
        .await
        .map_err(|e| turaes_core::Error::Unauthorized(format!("register failed: {}", e.message())))?
        .into_inner();
    let agent_token = registered.agent_token;
    println!(
        "registered as '{}' (server {})",
        args.name, registered.server_id
    );

    let store = ArtifactStore::new(&cfg.runtime.artifact_dir);
    let http = reqwest::Client::new();
    let http_base = args.http.trim_end_matches('/').to_string();
    let interval = Duration::from_secs(args.interval.max(5));

    // name -> applied artifact hash
    let mut applied: HashMap<String, String> = HashMap::new();

    loop {
        let _ = client
            .heartbeat(pb::HeartbeatRequest {
                agent_token: agent_token.clone(),
            })
            .await;

        match client
            .poll(pb::PollRequest {
                agent_token: agent_token.clone(),
            })
            .await
        {
            Ok(resp) => {
                for app in resp.into_inner().apps {
                    if app.artifact_hash.is_empty() {
                        continue; // never deployed
                    }
                    if applied.get(&app.name) == Some(&app.artifact_hash) {
                        continue; // unchanged
                    }
                    let outcome =
                        reconcile(&cfg, &store, &http, &http_base, &agent_token, &app).await;
                    let (state, message) = match outcome {
                        Ok(state) => {
                            applied.insert(app.name.clone(), app.artifact_hash.clone());
                            (state, String::new())
                        }
                        Err(e) => ("failed".to_string(), e.to_string()),
                    };
                    let _ = client
                        .report(pb::ReportRequest {
                            agent_token: agent_token.clone(),
                            app_name: app.name.clone(),
                            status: state,
                            artifact_hash: app.artifact_hash.clone(),
                            message,
                        })
                        .await;
                }
            }
            Err(e) => tracing::warn!(error = %e.message(), "poll failed"),
        }

        tokio::time::sleep(interval).await;
    }
}

/// Ensure the artifact is present, then install + restart the app.
async fn reconcile(
    cfg: &Config,
    store: &ArtifactStore,
    http: &reqwest::Client,
    http_base: &str,
    agent_token: &str,
    app: &pb::DesiredApp,
) -> Result<String> {
    let hash = &app.artifact_hash;
    if !store.has(hash) {
        let url = format!("{http_base}/agent/artifacts/{hash}");
        let bytes = http
            .get(&url)
            .bearer_auth(agent_token)
            .send()
            .await
            .map_err(|e| turaes_core::Error::Internal(format!("artifact fetch failed: {e}")))?
            .error_for_status()
            .map_err(|e| turaes_core::Error::Internal(format!("artifact fetch {url}: {e}")))?
            .bytes()
            .await
            .map_err(|e| turaes_core::Error::Internal(format!("artifact body: {e}")))?;
        store.store_bytes(hash, &bytes).await?;
        tracing::info!(app = %app.name, hash = %hash, "artifact fetched");
    }

    let spec = AppSpec {
        name: app.name.clone(),
        binary_path: store.path_for(hash)?.to_string_lossy().to_string(),
        installed_path: format!("{}/{}", cfg.runtime.bin_dir, app.name),
        args: if app.args.trim().is_empty() {
            None
        } else {
            Some(app.args.clone())
        },
        port: app.port as u16,
        state_dir: format!("{}/{}", cfg.runtime.state_dir, app.name),
        env_file: Some(format!("{}/{}.env", cfg.runtime.env_dir, app.name)),
        user: None,
    };
    let env: BTreeMap<String, String> = app.env.clone().into_iter().collect();
    let runtime = runtime_for(cfg, &app.runtime);
    let out = Deployer::new(runtime).deploy(&spec, &env).await?;
    Ok(out.state.as_status().to_string())
}
