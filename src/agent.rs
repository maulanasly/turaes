//! `turaes agent` — dials the control plane, registers, and heartbeats.
//!
//! N1b scaffolding: registration + liveness. App reconciliation (install
//! artifact, render unit) over the desired-state channel lands in N1c.

use std::time::Duration;

use turaes_core::Result;

use crate::grpc::pb;

/// Arguments for the agent role.
#[derive(Debug, Clone)]
pub struct AgentArgs {
    /// Control-plane gRPC endpoint, e.g. `http://10.0.0.2:9443`.
    pub control: String,
    /// Shared join token.
    pub token: String,
    /// Node name (unique in the fleet).
    pub name: String,
    /// Reachable address advertised to the control plane.
    pub address: String,
    /// Heartbeat interval, seconds.
    pub interval: u64,
}

/// Register and then heartbeat forever.
pub async fn run(args: AgentArgs) -> Result<()> {
    let mut client = pb::control_client::ControlClient::connect(args.control.clone())
        .await
        .map_err(|e| {
            turaes_core::Error::Internal(format!("failed to connect to {}: {e}", args.control))
        })?;

    let registered = client
        .register(pb::RegisterRequest {
            join_token: args.token,
            name: args.name.clone(),
            address: args.address.clone(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        })
        .await
        .map_err(|e| turaes_core::Error::Unauthorized(format!("register failed: {}", e.message())))?
        .into_inner();

    println!(
        "registered as '{}' (server {})",
        args.name, registered.server_id
    );
    tracing::info!(server_id = %registered.server_id, name = %args.name, "agent registered");

    let interval = Duration::from_secs(args.interval.max(5));
    loop {
        match client
            .heartbeat(pb::HeartbeatRequest {
                agent_token: registered.agent_token.clone(),
            })
            .await
        {
            Ok(resp) => tracing::debug!(server_id = %resp.into_inner().server_id, "heartbeat ok"),
            Err(e) => tracing::warn!(error = %e.message(), "heartbeat failed"),
        }
        tokio::time::sleep(interval).await;
    }
}
