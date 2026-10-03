//! `turaes edge` — a standalone Pingora edge that pulls its route table and
//! certificate material from the control plane. The control plane owns the
//! data; the edge is stateless and restartable.
//!
//! Requires a build with `--features proxy`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use turaes_core::config::Config;
use turaes_core::Result;
use turaes_proxy::{RouteTable, Router, Upstream};

use crate::grpc::pb;

/// Arguments for the edge role.
#[derive(Debug, Clone)]
pub struct EdgeArgs {
    /// Control-plane gRPC endpoint, e.g. `http://10.0.0.2:9443`.
    pub control: String,
    /// Shared control token (same secret as the agent join token).
    pub token: String,
    /// Route/cert refresh interval, seconds.
    pub interval: u64,
}

/// Start the proxy and poll the control plane for routes + certs.
pub async fn run(cfg: Arc<Config>, args: EdgeArgs) -> Result<()> {
    if !turaes_proxy::pingora_enabled() {
        return Err(turaes_core::Error::Config(
            "the edge role requires a build with `--features proxy`".into(),
        ));
    }

    // Best-effort: fetch certs before starting the proxy so TLS can come up.
    // Retries briefly if the control plane isn't ready yet.
    for _ in 0..10 {
        match fetch_certs(&args.control, &args.token).await {
            Ok(certs) if !certs.is_empty() => {
                write_certs(&cfg.proxy.cert_dir, certs).await;
                break;
            }
            _ => tokio::time::sleep(Duration::from_secs(2)).await,
        }
    }

    let router = Arc::new(Router::new(cfg.server.base_domain.clone(), HashMap::new()));
    let proxy_state = turaes_proxy::state(&cfg.proxy, router.clone(), cfg.dashboard_host());
    turaes_proxy::service::spawn(cfg.proxy.clone(), proxy_state);
    tracing::info!(
        http_port = cfg.proxy.http_port,
        https_port = cfg.proxy.https_port,
        "edge proxy started"
    );

    let interval = Duration::from_secs(args.interval.max(5));

    // Keep serving even if the control plane is temporarily unreachable:
    // reconnect and keep the last published routes/certs.
    loop {
        let mut client = match pb::control_client::ControlClient::connect(args.control.clone())
            .await
        {
            Ok(client) => client,
            Err(e) => {
                tracing::warn!(control = %args.control, error = %e, "edge: control plane unreachable; retrying");
                tokio::time::sleep(interval).await;
                continue;
            }
        };

        loop {
            match client
                .edge_routes(pb::EdgeRoutesRequest {
                    token: args.token.clone(),
                })
                .await
            {
                Ok(resp) => {
                    let mut routes: HashMap<String, Vec<Upstream>> = HashMap::new();
                    for r in resp.into_inner().routes {
                        routes
                            .entry(r.host.to_lowercase())
                            .or_default()
                            .push(Upstream {
                                host: r.address,
                                port: r.port as u16,
                                tls: r.tls,
                            });
                    }
                    let count = routes.len();
                    router.publish(RouteTable::new(cfg.server.base_domain.clone(), routes));
                    tracing::debug!(routes = count, "edge routes updated");

                    if let Ok(certs) = fetch_certs_with(&mut client, &args.token).await {
                        write_certs(&cfg.proxy.cert_dir, certs).await;
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e.message(), "edge: route fetch failed; reconnecting");
                    break;
                }
            }
            tokio::time::sleep(interval).await;
        }
        tokio::time::sleep(interval).await;
    }
}

async fn fetch_certs(control: &str, token: &str) -> Result<Vec<pb::EdgeCert>> {
    let mut client = pb::control_client::ControlClient::connect(control.to_string())
        .await
        .map_err(|e| turaes_core::Error::Internal(format!("connect {control}: {e}")))?;
    fetch_certs_with(&mut client, token).await
}

async fn fetch_certs_with(
    client: &mut pb::control_client::ControlClient<tonic::transport::Channel>,
    token: &str,
) -> Result<Vec<pb::EdgeCert>> {
    let resp = client
        .edge_certs(pb::EdgeCertsRequest {
            token: token.to_string(),
        })
        .await
        .map_err(|e| turaes_core::Error::Internal(format!("edge_certs: {}", e.message())))?;
    Ok(resp.into_inner().certs)
}

/// Write cert material under `cert_dir` (only when changed, so the proxy's
/// mtime-based reload picks it up).
async fn write_certs(cert_dir: &str, certs: Vec<pb::EdgeCert>) {
    for cert in certs {
        let dir = Path::new(cert_dir).join(&cert.host);
        if tokio::fs::create_dir_all(&dir).await.is_err() {
            continue;
        }
        let _ = write_if_changed(&dir.join("fullchain.pem"), &cert.fullchain_pem).await;
        let _ = write_if_changed(&dir.join("privkey.pem"), &cert.privkey_pem).await;
    }
}

async fn write_if_changed(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Ok(existing) = tokio::fs::read(path).await {
        if existing == bytes {
            return Ok(());
        }
    }
    tokio::fs::write(path, bytes).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(mut perms) = tokio::fs::metadata(path).await.map(|m| m.permissions()) {
            perms.set_mode(0o600);
            let _ = tokio::fs::set_permissions(path, perms).await;
        }
    }
    Ok(())
}
