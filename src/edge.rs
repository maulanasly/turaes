//! `turaes edge` — a standalone Pingora edge that pulls its route table from the
//! control plane. The control plane owns the data; the edge is stateless and
//! restartable. TLS certs are loaded from the local certbot layout (N4 adds
//! multi-cert SNI + cert distribution).
//!
//! Requires a build with `--features proxy`.

use std::collections::HashMap;
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
    /// Route refresh interval, seconds.
    pub interval: u64,
}

/// Start the proxy and poll the control plane for routes.
pub async fn run(cfg: Arc<Config>, args: EdgeArgs) -> Result<()> {
    if !turaes_proxy::pingora_enabled() {
        return Err(turaes_core::Error::Config(
            "the edge role requires a build with `--features proxy`".into(),
        ));
    }

    let router = Arc::new(Router::new(cfg.server.base_domain.clone(), HashMap::new()));
    let proxy_state = turaes_proxy::state(&cfg.proxy, router.clone(), cfg.dashboard_host());
    turaes_proxy::service::spawn(cfg.proxy.clone(), proxy_state);
    tracing::info!(
        http_port = cfg.proxy.http_port,
        https_port = cfg.proxy.https_port,
        "edge proxy started"
    );

    let mut client = pb::control_client::ControlClient::connect(args.control.clone())
        .await
        .map_err(|e| {
            turaes_core::Error::Internal(format!("failed to connect to {}: {e}", args.control))
        })?;

    let interval = Duration::from_secs(args.interval.max(5));
    loop {
        match client
            .edge_routes(pb::EdgeRoutesRequest {
                token: args.token.clone(),
            })
            .await
        {
            Ok(resp) => {
                let mut routes = HashMap::new();
                for r in resp.into_inner().routes {
                    routes.insert(
                        r.host.to_lowercase(),
                        Upstream {
                            host: r.address,
                            port: r.port as u16,
                            tls: r.tls,
                        },
                    );
                }
                let count = routes.len();
                router.publish(RouteTable::new(cfg.server.base_domain.clone(), routes));
                tracing::debug!(routes = count, "edge routes updated");
            }
            Err(e) => tracing::warn!(error = %e.message(), "edge route fetch failed"),
        }
        tokio::time::sleep(interval).await;
    }
}
