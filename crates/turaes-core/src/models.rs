//! Database-backed domain models.
//!
//! Timestamps are RFC3339 text (SQLite `datetime('now')`) to keep the schema
//! portable and dependency-light. Ids are UUIDv4 strings.

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// A deployable application managed by turaes.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Application {
    /// UUID.
    pub id: String,
    /// Human name / slug (also the systemd unit and install name).
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Absolute path to the prebuilt binary on the server.
    pub binary_path: String,
    /// Optional command-line arguments.
    pub args: Option<String>,
    /// Port the process listens on (loopback).
    pub port: i64,
    /// Health check path, e.g. `/health`.
    pub health_path: String,
    /// Metrics path (tonggeret Prometheus exposition), e.g. `/metrics`.
    pub metrics_path: Option<String>,
    /// Primary hostname routed by the proxy.
    pub domain: Option<String>,
    /// `systemd` or `proc`.
    pub runtime: String,
    /// Restart on unhealthy.
    pub auto_restart: bool,
    /// `stopped`, `starting`, `running`, `unhealthy`, `failed`.
    pub status: String,
    /// Creation timestamp (RFC3339).
    pub created_at: String,
    /// Update timestamp (RFC3339).
    pub updated_at: String,
}

/// One historical deploy of an application.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Deployment {
    /// UUID.
    pub id: String,
    /// Owning application.
    pub application_id: String,
    /// `queued`, `installing`, `starting`, `running`, `failed`, `rolled_back`.
    pub status: String,
    /// Content hash of the installed artifact.
    pub artifact_hash: Option<String>,
    /// Captured build/install log.
    pub log: Option<String>,
    /// Path of the artifact replaced by this deploy (for rollback).
    pub previous_artifact: Option<String>,
    /// Start timestamp.
    pub started_at: String,
    /// Finish timestamp.
    pub finished_at: Option<String>,
}

/// Encrypted environment variable for an application.
#[derive(Debug, Clone, FromRow)]
pub struct EnvVar {
    /// UUID.
    pub id: String,
    /// Owning application.
    pub application_id: String,
    /// Variable name.
    pub key: String,
    /// AES-256-GCM sealed value (base64).
    pub value_enc: String,
    /// Creation timestamp.
    pub created_at: String,
}

/// Hostname mapped to an application.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Domain {
    /// UUID.
    pub id: String,
    /// Owning application.
    pub application_id: String,
    /// FQDN, e.g. `kalkulator.rayakala.ink`.
    pub domain: String,
    /// Is this the primary domain?
    pub is_primary: bool,
    /// Whether a matching certificate is present and loaded.
    pub ssl_active: bool,
    /// Creation timestamp.
    pub created_at: String,
}

/// Health check configuration for an application.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct HealthCheck {
    /// UUID.
    pub id: String,
    /// Owning application.
    pub application_id: String,
    /// Request path.
    pub path: String,
    /// Seconds between probes.
    pub interval_seconds: i64,
    /// Request timeout in seconds.
    pub timeout_seconds: i64,
    /// Consecutive successes required to be healthy.
    pub healthy_threshold: i64,
    /// Consecutive failures required to be unhealthy.
    pub unhealthy_threshold: i64,
    /// Creation timestamp.
    pub created_at: String,
}

/// Result of one health probe.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct HealthResult {
    /// UUID.
    pub id: String,
    /// Owning application.
    pub application_id: String,
    /// `healthy` or `unhealthy`.
    pub status: String,
    /// HTTP status code, when a response was received.
    pub status_code: Option<i64>,
    /// Round-trip time in milliseconds.
    pub response_time_ms: Option<i64>,
    /// Error message on failure.
    pub error_message: Option<String>,
    /// Check timestamp.
    pub checked_at: String,
}

/// One sampled resource reading for an application.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AppMetric {
    /// UUID.
    pub id: String,
    /// Owning application.
    pub application_id: String,
    /// CPU percent (0-100*ncores).
    pub cpu_pct: f64,
    /// Resident memory in bytes.
    pub mem_bytes: i64,
    /// Sample timestamp.
    pub recorded_at: String,
}

/// One visitor rollup bucket for an application/region.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct VisitMetric {
    /// UUID.
    pub id: String,
    /// Owning application.
    pub application_id: String,
    /// Region label (`unknown` when no CDN header).
    pub region: String,
    /// Cumulative or bucket visit count.
    pub visits: i64,
    /// Latest unique-visitor estimate for the bucket.
    pub uniques: i64,
    /// Bucket timestamp.
    pub recorded_at: String,
}

/// Audit / timeline event.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Event {
    /// UUID.
    pub id: String,
    /// Related application, when any.
    pub application_id: Option<String>,
    /// e.g. `deploy`, `health`, `restart`.
    pub kind: String,
    /// Human-readable message.
    pub message: String,
    /// Event timestamp.
    pub created_at: String,
}
