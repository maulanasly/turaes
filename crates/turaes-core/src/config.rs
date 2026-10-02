//! Layered configuration: `config/default.toml` (embedded) or a file on disk,
//! then `TURAES_*` environment overrides.
//!
//! Precedence, lowest to highest: embedded defaults → file → environment.

use std::path::Path;

use serde::Deserialize;

use crate::error::{Error, Result};

/// Embedded fallback so turaes boots without a config file on disk.
const EMBEDDED_DEFAULT: &str = include_str!("../../../config/default.toml");

/// Root configuration object.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// HTTP server settings.
    pub server: ServerConfig,
    /// Storage settings.
    pub database: DatabaseConfig,
    /// Authentication settings.
    pub auth: AuthConfig,
    /// Reverse-proxy settings.
    pub proxy: ProxyConfig,
    /// Monitoring settings.
    pub monitor: MonitorConfig,
    /// Process runtime settings.
    pub runtime: RuntimeConfig,
    /// gRPC control-plane (agent/edge channel) settings.
    #[serde(default)]
    pub grpc: GrpcConfig,
    /// Agent join settings.
    #[serde(default)]
    pub agent: AgentConfig,
}

/// gRPC listener for agents and the edge.
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct GrpcConfig {
    /// Whether the gRPC server starts with turaes.
    #[serde(default)]
    pub enabled: bool,
    /// Bind host.
    #[serde(default = "default_grpc_host")]
    pub host: String,
    /// Bind port.
    #[serde(default = "default_grpc_port")]
    pub port: u16,
}

fn default_grpc_host() -> String {
    "0.0.0.0".to_string()
}
fn default_grpc_port() -> u16 {
    9443
}

/// Agent registration settings.
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct AgentConfig {
    /// Shared secret an agent presents to `Register`.
    #[serde(default)]
    pub join_token: String,
}

/// HTTP listener for the dashboard and API.
#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    /// Bind address.
    pub host: String,
    /// Bind port.
    pub port: u16,
    /// Base domain used to derive app subdomains.
    pub base_domain: String,
    /// Public dashboard URL (used in redirects and OAuth).
    pub public_url: String,
}

/// SQLite connection settings.
#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    /// SQLx SQLite URL, e.g. `sqlite://turaes.db?mode=rwc`.
    pub url: String,
}

/// GitHub OAuth allowlist + session settings.
#[derive(Debug, Clone, Deserialize)]
pub struct AuthConfig {
    /// JWT signing secret (>= 32 chars in production).
    pub jwt_secret: String,
    /// GitHub OAuth app client id.
    pub github_client_id: String,
    /// GitHub OAuth app client secret.
    pub github_client_secret: String,
    /// Numeric GitHub user ids allowed to sign in.
    pub allowed_github_ids: Vec<i64>,
    /// Canonical dashboard origin, e.g. `https://turaes.example.com`.
    pub app_origin: String,
    /// Sliding session lifetime in days.
    pub session_ttl_days: i64,
    /// Force `Secure` cookies even on non-https origins.
    pub allow_insecure_cookies: bool,
}

/// Pingora reverse-proxy settings.
#[derive(Debug, Clone, Deserialize)]
pub struct ProxyConfig {
    /// Whether the in-process proxy starts with the server.
    pub enabled: bool,
    /// Plain HTTP listen port.
    pub http_port: u16,
    /// HTTPS listen port.
    pub https_port: u16,
    /// Directory holding certbot per-domain certificate folders.
    pub cert_dir: String,
    /// Hostname routed to the turaes dashboard itself. Defaults to the host of
    /// `server.public_url` when unset.
    #[serde(default)]
    pub dashboard_host: Option<String>,
}

/// Health and metrics collection settings.
#[derive(Debug, Clone, Deserialize)]
pub struct MonitorConfig {
    /// Seconds between health checks and metric scrapes.
    pub interval_secs: u64,
    /// Days of samples to retain.
    pub retention_days: i64,
    /// Paths excluded from visitor counting.
    pub visitor_skip_paths: Vec<String>,
}

/// Native process runtime settings.
#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeConfig {
    /// `systemd` (default) or `proc` (embedded supervisor).
    pub driver: String,
    /// systemd unit directory.
    pub unit_dir: String,
    /// Directory where app binaries are installed.
    pub bin_dir: String,
    /// Root for per-app state (`{state_dir}/{app}`).
    pub state_dir: String,
    /// Directory for per-app environment files (`{env_dir}/{app}.env`).
    pub env_dir: String,
    /// Content-addressed artifact store root (`{artifact_dir}/sha256/{hex}`).
    #[serde(default = "default_artifact_dir")]
    pub artifact_dir: String,
}

fn default_artifact_dir() -> String {
    "/var/lib/turaes/artifacts".to_string()
}

impl Config {
    /// Load configuration: optional file, then environment overrides, then validate.
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let mut cfg: Config = match path {
            Some(p) if p.exists() => {
                let raw = std::fs::read_to_string(p)
                    .map_err(|e| Error::Config(format!("failed to read {}: {e}", p.display())))?;
                toml::from_str(&raw)
                    .map_err(|e| Error::Config(format!("invalid TOML in {}: {e}", p.display())))?
            }
            _ => toml::from_str(EMBEDDED_DEFAULT)
                .map_err(|e| Error::Config(format!("embedded config invalid: {e}")))?,
        };
        apply_env(&mut cfg)?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Parse configuration from a TOML string (no environment overrides).
    ///
    /// Useful for tests and for embedding a config blob. Still validated.
    pub fn from_toml(raw: &str) -> Result<Self> {
        let cfg: Config =
            toml::from_str(raw).map_err(|e| Error::Config(format!("invalid TOML: {e}")))?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Boot-time validation. Release builds refuse the placeholder secret and
    /// insecure non-localhost cookies.
    pub fn validate(&self) -> Result<()> {
        if self.auth.jwt_secret.len() < 32 {
            // Debug builds may run with the placeholder for convenience.
            if !cfg!(debug_assertions) {
                return Err(Error::Config(
                    "TURAES_JWT_SECRET must be at least 32 characters".into(),
                ));
            }
            tracing::warn!("jwt_secret is shorter than 32 characters; fine for dev only");
        }
        if !self.auth.allow_insecure_cookies && !self.is_secure_origin() {
            return Err(Error::Config(format!(
                "APP_ORIGIN '{}' is not https; set allow_insecure_cookies = true to override",
                self.auth.app_origin
            )));
        }
        if !matches!(self.runtime.driver.as_str(), "systemd" | "proc") {
            return Err(Error::Config(format!(
                "runtime.driver must be 'systemd' or 'proc', got '{}'",
                self.runtime.driver
            )));
        }
        Ok(())
    }

    /// True when the configured origin is https or a localhost origin.
    pub fn is_secure_origin(&self) -> bool {
        let origin = &self.auth.app_origin;
        origin.starts_with("https://")
            || origin.starts_with("http://localhost")
            || origin.starts_with("http://127.0.0.1")
            || origin.starts_with("http://[::1]")
    }

    /// Hostname the proxy routes to the turaes dashboard itself.
    ///
    /// Uses `proxy.dashboard_host` when set; otherwise derives the host from
    /// `server.public_url` (scheme and port stripped).
    pub fn dashboard_host(&self) -> Option<String> {
        if let Some(host) = self.proxy.dashboard_host.as_deref() {
            if !host.is_empty() {
                return Some(host.to_string());
            }
        }
        let after_scheme = self
            .server
            .public_url
            .split("://")
            .nth(1)
            .unwrap_or(&self.server.public_url);
        let host = after_scheme
            .split('/')
            .next()
            .unwrap_or("")
            .split(':')
            .next()
            .unwrap_or("");
        if host.is_empty() {
            None
        } else {
            Some(host.to_string())
        }
    }

    /// The OAuth callback URL registered with GitHub.
    pub fn callback_url(&self) -> String {
        format!(
            "{}/auth/callback",
            self.auth.app_origin.trim_end_matches('/')
        )
    }

    /// Whether the server should set `Secure` on session cookies.
    pub fn secure_cookies(&self) -> bool {
        self.is_secure_origin()
    }
}

fn env_str(key: &str, target: &mut String) {
    if let Ok(v) = std::env::var(key) {
        if !v.is_empty() {
            *target = v;
        }
    }
}

fn env_parse<T: std::str::FromStr>(key: &str, target: &mut T) {
    if let Ok(v) = std::env::var(key) {
        if let Ok(parsed) = v.parse::<T>() {
            *target = parsed;
        } else {
            tracing::warn!(key, value = %v, "ignoring unparseable env override");
        }
    }
}

fn env_bool(key: &str, target: &mut bool) {
    if let Ok(v) = std::env::var(key) {
        match v.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => *target = true,
            "0" | "false" | "no" | "off" => *target = false,
            _ => tracing::warn!(key, value = %v, "ignoring unparseable bool override"),
        }
    }
}

fn apply_env(cfg: &mut Config) -> Result<()> {
    env_str("TURAES_HOST", &mut cfg.server.host);
    env_parse("TURAES_PORT", &mut cfg.server.port);
    env_str("TURAES_BASE_DOMAIN", &mut cfg.server.base_domain);
    env_str("TURAES_PUBLIC_URL", &mut cfg.server.public_url);

    env_str("TURAES_DATABASE_URL", &mut cfg.database.url);

    env_str("TURAES_JWT_SECRET", &mut cfg.auth.jwt_secret);
    env_str("TURAES_GITHUB_CLIENT_ID", &mut cfg.auth.github_client_id);
    env_str(
        "TURAES_GITHUB_CLIENT_SECRET",
        &mut cfg.auth.github_client_secret,
    );
    env_str("TURAES_APP_ORIGIN", &mut cfg.auth.app_origin);
    env_parse("TURAES_SESSION_TTL_DAYS", &mut cfg.auth.session_ttl_days);
    env_bool(
        "TURAES_ALLOW_INSECURE_COOKIES",
        &mut cfg.auth.allow_insecure_cookies,
    );
    if let Ok(raw) = std::env::var("TURAES_ALLOWED_GITHUB_IDS") {
        let ids: Vec<i64> = raw
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .filter_map(|s| match s.parse::<i64>() {
                Ok(id) => Some(id),
                Err(_) => {
                    tracing::warn!(value = %s, "ignoring non-numeric allowed github id");
                    None
                }
            })
            .collect();
        cfg.auth.allowed_github_ids = ids;
    }

    env_bool("TURAES_PROXY_ENABLED", &mut cfg.proxy.enabled);
    env_parse("TURAES_PROXY_HTTP_PORT", &mut cfg.proxy.http_port);
    env_parse("TURAES_PROXY_HTTPS_PORT", &mut cfg.proxy.https_port);
    env_str("TURAES_CERT_DIR", &mut cfg.proxy.cert_dir);
    if let Ok(host) = std::env::var("TURAES_PROXY_DASHBOARD_HOST") {
        cfg.proxy.dashboard_host = Some(host);
    }

    env_parse(
        "TURAES_MONITOR_INTERVAL_SECS",
        &mut cfg.monitor.interval_secs,
    );
    env_parse(
        "TURAES_MONITOR_RETENTION_DAYS",
        &mut cfg.monitor.retention_days,
    );

    env_str("TURAES_RUNTIME_DRIVER", &mut cfg.runtime.driver);
    env_str("TURAES_UNIT_DIR", &mut cfg.runtime.unit_dir);
    env_str("TURAES_BIN_DIR", &mut cfg.runtime.bin_dir);
    env_str("TURAES_STATE_DIR", &mut cfg.runtime.state_dir);
    env_str("TURAES_ENV_DIR", &mut cfg.runtime.env_dir);
    env_str("TURAES_ARTIFACT_DIR", &mut cfg.runtime.artifact_dir);

    env_bool("TURAES_GRPC_ENABLED", &mut cfg.grpc.enabled);
    env_str("TURAES_GRPC_HOST", &mut cfg.grpc.host);
    env_parse("TURAES_GRPC_PORT", &mut cfg.grpc.port);
    env_str("TURAES_AGENT_JOIN_TOKEN", &mut cfg.agent.join_token);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_default_parses() {
        let cfg: Config = toml::from_str(EMBEDDED_DEFAULT).expect("embedded default must parse");
        assert_eq!(cfg.server.port, 8787);
        assert_eq!(cfg.runtime.driver, "systemd");
        assert_eq!(cfg.monitor.interval_secs, 15);
    }

    #[test]
    fn localhost_origin_is_secure_enough() {
        let cfg: Config = toml::from_str(EMBEDDED_DEFAULT).unwrap();
        assert!(cfg.is_secure_origin());
        assert!(cfg.secure_cookies());
    }

    #[test]
    fn callback_url_trims_slash() {
        let mut cfg: Config = toml::from_str(EMBEDDED_DEFAULT).unwrap();
        cfg.auth.app_origin = "https://turaes.example.com/".into();
        assert_eq!(
            cfg.callback_url(),
            "https://turaes.example.com/auth/callback"
        );
    }
}
