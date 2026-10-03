//! `turaes agent` — reconcile desired state from the control plane.
//!
//! Loop: heartbeat → poll desired apps → fetch missing artifacts over HTTP →
//! install/restart via the local runtime → report status. Outbound only.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use turaes_core::artifact::ArtifactStore;
use turaes_core::config::Config;
use turaes_core::Result;
use turaes_monitor::{health, scrape, stats};
use turaes_runtime::{AppSpec, Deployer};

use crate::grpc::pb;
use crate::routes::apps::runtime_for;

/// Per-app sampling state (resource deltas + visitor counters).
#[derive(Default)]
struct MetricsMemo {
    last_cpu_usec: Option<u64>,
    last_tick: Option<Instant>,
    visit_counters: HashMap<String, i64>,
    health: health::Threshold,
}

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
    let mut memo: HashMap<String, MetricsMemo> = HashMap::new();

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
                let apps = resp.into_inner().apps;

                for app in &apps {
                    if app.artifact_hash.is_empty() {
                        continue; // never deployed
                    }
                    if applied.get(&app.name) == Some(&app.artifact_hash) {
                        continue; // unchanged
                    }
                    let outcome =
                        reconcile(&cfg, &store, &http, &http_base, &agent_token, app).await;
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

                // Sample + push metrics for every desired app (including ones
                // already applied).
                let mut metrics = Vec::new();
                for app in &apps {
                    // Health transition -> report status.
                    {
                        let hc = health::HealthConfig {
                            path: app.health_path.clone(),
                            ..Default::default()
                        };
                        let url = format!("http://127.0.0.1:{}{}", app.port, app.health_path);
                        let outcome =
                            health::probe(&http, &url, Duration::from_secs(hc.timeout_secs)).await;
                        let entry = memo.entry(app.name.clone()).or_default();
                        if let Some(t) = entry.health.record(&outcome, &hc) {
                            let status = match t {
                                health::Transition::BecameHealthy => "running",
                                health::Transition::BecameUnhealthy => "unhealthy",
                            };
                            let _ = client
                                .report(pb::ReportRequest {
                                    agent_token: agent_token.clone(),
                                    app_name: app.name.clone(),
                                    status: status.into(),
                                    artifact_hash: app.artifact_hash.clone(),
                                    message: outcome.error_message.clone().unwrap_or_default(),
                                })
                                .await;
                        }
                    }
                    if let Some(m) = sample_metrics(&cfg, &http, &mut memo, app).await {
                        metrics.push(m);
                    }
                }
                if !metrics.is_empty() {
                    let _ = client
                        .push_metrics(pb::MetricsRequest {
                            agent_token: agent_token.clone(),
                            apps: metrics,
                        })
                        .await;
                }
            }
            Err(e) => tracing::warn!(error = %e.message(), "poll failed"),
        }

        tokio::time::sleep(interval).await;
    }
}

/// Sample cgroup/proc resources and the app's `/metrics` into an AppMetrics.
async fn sample_metrics(
    cfg: &Config,
    http: &reqwest::Client,
    memo: &mut HashMap<String, MetricsMemo>,
    app: &pb::DesiredApp,
) -> Option<pb::AppMetrics> {
    let m = memo.entry(app.name.clone()).or_default();
    let elapsed = m
        .last_tick
        .map(|t| t.elapsed().as_secs_f64())
        .unwrap_or(0.0);
    m.last_tick = Some(Instant::now());

    let reading = match app.runtime.as_str() {
        "proc" => read_proc(cfg, &app.name).await,
        _ => {
            let dir = format!("/sys/fs/cgroup/system.slice/{}.service", app.name);
            stats::read_cgroup(Path::new(&dir)).await
        }
    };
    let (cpu_pct, mem_bytes) = match reading {
        Some(r) => {
            let cpu = match m.last_cpu_usec.replace(r.cpu_usage_usec) {
                Some(prev) => stats::cpu_percent(prev, r.cpu_usage_usec, elapsed),
                None => 0.0,
            };
            (cpu, r.mem_bytes)
        }
        None => (0.0, 0),
    };

    let mut visits = Vec::new();
    if !app.metrics_path.is_empty() {
        let url = format!("http://127.0.0.1:{}{}", app.port, app.metrics_path);
        if let Ok(resp) = http.get(&url).timeout(Duration::from_secs(5)).send().await {
            if resp.status().is_success() {
                if let Ok(body) = resp.text().await {
                    for v in scrape::visitor_samples(&scrape::parse(&body)) {
                        let prev = m.visit_counters.get(&v.region).copied().unwrap_or(0);
                        let delta = if v.visits >= prev {
                            v.visits - prev
                        } else {
                            v.visits
                        };
                        m.visit_counters.insert(v.region.clone(), v.visits);
                        if delta != 0 || v.uniques != 0 {
                            visits.push(pb::RegionVisits {
                                region: v.region,
                                visits: delta,
                                uniques: v.uniques,
                            });
                        }
                    }
                }
            }
        }
    }

    if reading.is_none() && visits.is_empty() {
        return None;
    }
    Some(pb::AppMetrics {
        app_name: app.name.clone(),
        cpu_pct,
        mem_bytes,
        visits,
    })
}

async fn read_proc(cfg: &Config, name: &str) -> Option<stats::ResourceStats> {
    let pid_path = format!("{}/{}/{}.pid", cfg.runtime.state_dir, name, name);
    let pid = tokio::fs::read_to_string(pid_path).await.ok()?;
    let body = tokio::fs::read_to_string(format!("/proc/{}/stat", pid.trim()))
        .await
        .ok()?;
    let parsed = stats::parse_proc_stat(&body)?;
    Some(stats::proc_to_stats(parsed, 100, 4096))
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
        slot: None,
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
