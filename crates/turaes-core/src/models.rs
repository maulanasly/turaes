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
    /// Port the process listens on (loopback). Slot A.
    pub port: i64,
    /// Port the proxy currently routes to (blue/green slot); falls back to `port`.
    pub active_port: Option<i64>,
    /// Health check path, e.g. `/health`.
    pub health_path: String,
    /// Metrics path (tonggeret Prometheus exposition), e.g. `/metrics`.
    pub metrics_path: Option<String>,
    /// Primary hostname routed by the proxy.
    pub domain: Option<String>,
    /// Node this app is placed on (`local` by default).
    pub server_id: String,
    /// Owning tenant (`default` until memberships move it).
    pub org_id: String,
    /// Resident memory ceiling in MiB (systemd `MemoryMax=`); unset = unlimited.
    pub mem_limit_mb: Option<i64>,
    /// CPU ceiling in percent of one core (systemd `CPUQuota=`); unset = unlimited.
    pub cpu_quota_pct: Option<i64>,
    /// `service` (default), `static` (publish a directory), or `worker`
    /// (supervised background process, no HTTP surface).
    pub kind: String,
    /// JSON argv array for interpreted apps; mutually exclusive with the
    /// single-binary path at the manifest layer.
    pub command: Option<String>,
    /// WorkingDirectory override; defaults to `{state_dir}/{app}`.
    pub workdir: Option<String>,
    /// Source directory synced for `static` apps.
    pub publish_dir: Option<String>,
    /// `systemd` or `proc`.
    pub runtime: String,
    /// Restart on unhealthy.
    pub auto_restart: bool,
    /// `stopped`, `starting`, `running`, `unhealthy`, `failed`.
    pub status: String,
    /// Explicit maintenance mode: proxy parks the hostnames (503 page)
    /// while units keep running. Independent of supervisor status.
    pub maintenance: bool,
    /// Creation timestamp (RFC3339).
    pub created_at: String,
    /// Update timestamp (RFC3339).
    pub updated_at: String,
}

/// A node in the fleet. The local host is the row with id `local`.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Server {
    /// UUID, or `local` for this host.
    pub id: String,
    /// Human name (unique).
    pub name: String,
    /// Reachable address (private IP in the VPC; `127.0.0.1` for local).
    pub address: String,
    /// SSH host for bootstrap, when remote.
    pub ssh_host: Option<String>,
    /// SSH port.
    pub ssh_port: Option<i64>,
    /// SSH user.
    pub ssh_user: Option<String>,
    /// Sealed private key (never serialized).
    #[serde(skip_serializing)]
    pub ssh_key_enc: Option<String>,
    /// Whether this is the control-plane host.
    pub is_local: bool,
    /// `online`, `offline`, `unknown`.
    pub status: String,
    /// Last heartbeat (RFC3339).
    pub last_seen_at: Option<String>,
    /// Reported agent version, when present.
    pub agent_version: Option<String>,
    /// Creation timestamp.
    pub created_at: String,
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

/// One sampled host resource reading for a server (local control plane only).
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ServerMetric {
    /// UUID.
    pub id: String,
    /// Owning server (`local` for control-plane self-samples).
    pub server_id: String,
    /// CPU percent, normalized 0-100 across all cores (unlike per-app
    /// readings, which are not normalised, so bars stay bounded).
    pub cpu_pct: f64,
    /// Used memory in bytes (`MemTotal - MemAvailable`).
    pub mem_bytes: i64,
    /// Total memory in bytes (so the UI can render `used/total`).
    pub mem_total_bytes: i64,
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

/// Resource quotas bounding what an organization may claim.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct OrgQuota {
    /// Owning organization.
    pub org_id: String,
    /// Maximum applications.
    pub max_apps: i64,
    /// Maximum summed `mem_limit_mb` across the org's apps.
    pub max_mem_mb: i64,
    /// Maximum summed `cpu_quota_pct` across the org's apps.
    pub max_cpu_pct: i64,
    /// Maximum hostnames (primary domains + aliases).
    pub max_domains: i64,
}

/// A firing or resolved alert from the monitor's rule evaluation.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Alert {
    /// UUID.
    pub id: String,
    /// Owning organization; `None` for platform-global alerts.
    pub org_id: Option<String>,
    /// `warning` or `critical`.
    pub severity: String,
    /// Rule name, e.g. `app.unhealthy`.
    pub kind: String,
    /// Deduplication key, e.g. `app.unhealthy:{app_id}`.
    pub key: String,
    /// Short human summary.
    pub subject: String,
    /// Longer context, when any.
    pub detail: Option<String>,
    /// `firing` or `resolved`.
    pub status: String,
    /// Related application, when any.
    pub application_id: Option<String>,
    /// First fired timestamp.
    pub fired_at: String,
    /// Resolution timestamp, when resolved.
    pub resolved_at: Option<String>,
    /// Last notification dispatch, when any.
    pub notified_at: Option<String>,
    /// Creation timestamp.
    pub created_at: String,
}

/// A tenant workspace.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Organization {
    /// UUID (the seeded default org is `default`).
    pub id: String,
    /// URL-safe unique slug.
    pub slug: String,
    /// Display name.
    pub name: String,
    /// Creation timestamp.
    pub created_at: String,
}

/// A signed-in principal (persisted on first OAuth login).
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct User {
    /// UUID.
    pub id: String,
    /// Numeric GitHub user id (unique).
    pub github_id: i64,
    /// GitHub login.
    pub login: String,
    /// Display name from GitHub, when present.
    pub name: Option<String>,
    /// Creation timestamp.
    pub created_at: String,
    /// Last seen timestamp.
    pub last_seen_at: Option<String>,
}

/// A user's role within an organization.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Membership {
    /// UUID.
    pub id: String,
    /// Owning organization.
    pub org_id: String,
    /// Member user.
    pub user_id: String,
    /// `owner`, `admin`, `developer` or `viewer`.
    pub role: String,
    /// Creation timestamp.
    pub created_at: String,
}

/// A hashed, scoped token for programmatic/CI access.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ApiToken {
    /// UUID.
    pub id: String,
    /// Owning organization.
    pub org_id: String,
    /// Creating user, when still present.
    pub user_id: Option<String>,
    /// Human name for the token.
    pub name: String,
    /// SHA-256 hash of the plaintext token (plaintext is never stored).
    #[serde(skip_serializing)]
    pub token_hash: String,
    /// Comma-separated scopes (`read`, `deploy`, `admin`).
    pub scopes: String,
    /// Last use timestamp.
    pub last_used_at: Option<String>,
    /// Creation timestamp.
    pub created_at: String,
    /// Revocation timestamp, when revoked.
    pub revoked_at: Option<String>,
}

/// One durable audit record of a mutating action.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AuditLog {
    /// UUID.
    pub id: String,
    /// Owning organization, when known.
    pub org_id: Option<String>,
    /// Acting user, when known (absent for system actions).
    pub actor_user_id: Option<String>,
    /// Related application, when any.
    pub application_id: Option<String>,
    /// Verb, e.g. `app.deploy`, `env.set`, `token.create`.
    pub action: String,
    /// Target kind, e.g. `application`, `domain`, `token`.
    pub target_type: Option<String>,
    /// Target identifier.
    pub target_id: Option<String>,
    /// Optional JSON metadata (never secrets).
    pub metadata: Option<String>,
    /// Event timestamp.
    pub created_at: String,
}

/// A curated starter template for the Catalog page.
///
/// Templates are platform-global (identical for every org). `defaults_json`
/// keys map onto wizard draft fields; unknown keys are ignored by the UI.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct CatalogTemplate {
    /// Stable id (`tmpl_<slug>` for seeds).
    pub id: String,
    /// URL-safe key used by `?template=<slug>` deep links.
    pub slug: String,
    /// Display name.
    pub name: String,
    /// One-line blurb.
    pub description: String,
    /// `service`, `static`, or `worker`.
    pub kind: String,
    /// JSON object of wizard draft defaults.
    pub defaults_json: String,
    /// Display order (ascending).
    pub sort_order: i64,
    /// Creation timestamp.
    pub created_at: String,
}

/// A CI repo linked to an app: pushes claiming this repo may land here.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct RegistryLink {
    /// UUID.
    pub id: String,
    /// Owning organization.
    pub org_id: String,
    /// Linked application.
    pub application_id: String,
    /// CI repository (`owner/name`).
    pub repo: String,
    /// Linking user, when known.
    pub created_by: Option<String>,
    /// Creation timestamp.
    pub created_at: String,
}

/// Metadata for one stored blob. Bytes live in the `ArtifactStore` on disk.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct RegistryArtifact {
    /// UUID.
    pub id: String,
    /// Owning organization.
    pub org_id: String,
    /// Content hash (`sha256:<hex>`).
    pub hash: String,
    /// Blob size in bytes.
    pub size_bytes: i64,
    /// MIME type (`application/octet-stream` for ELF binaries).
    pub media_type: String,
    /// CPU architecture (`x86_64`, `aarch64`), when known.
    pub arch: Option<String>,
    /// Uploading user, when known.
    pub created_by: Option<String>,
    /// Creation timestamp.
    pub created_at: String,
}

/// An immutable named pointer to one artifact (plus bundle members).
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Release {
    /// UUID.
    pub id: String,
    /// Owning organization.
    pub org_id: String,
    /// Owning application.
    pub application_id: String,
    /// Strict semver (`1.2.3`, never a channel name).
    pub version: String,
    /// Primary artifact row.
    pub artifact_id: String,
    /// Source commit, when reported by CI.
    pub commit_sha: Option<String>,
    /// CI run URL, when reported.
    pub build_url: Option<String>,
    /// Free-form release notes.
    pub notes: Option<String>,
    /// Bundle members (`[{path, hash, mode}]`); first entry deploys.
    pub files_json: String,
    /// Yanked releases resolve to 404 but keep their bytes.
    pub is_yanked: bool,
    /// Uploading user, when known.
    pub created_by: Option<String>,
    /// Creation timestamp.
    pub created_at: String,
}
