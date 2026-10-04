//! `turaes.yaml` application manifest: the declarative front-end to
//! applications and their related tables.
//!
//! One file declares one app. Field validation here mirrors the HTTP API rules
//! so CLI and dashboard can never disagree; resource/port/quota conflicts are
//! checked at apply time against live state, not here.
//!
//! Secrets are names-only in the file. Values live in the sealed store,
//! written out-of-band via `turaes secrets set` (or the dashboard env editor).

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::error::{Error, Result};

/// Application kind: `service` (default, long-lived HTTP process),
/// `static` (publish a directory, always healthy), or `worker` (supervised
/// background process with no HTTP surface).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppKind {
    #[default]
    Service,
    Static,
    Worker,
}

impl AppKind {
    /// Canonical lowercase name for storage.
    pub fn as_str(self) -> &'static str {
        match self {
            AppKind::Service => "service",
            AppKind::Static => "static",
            AppKind::Worker => "worker",
        }
    }
}

/// A duration: `15s`, `5m`, `1h`, or bare seconds (`15`).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum DurationSpec {
    /// Bare seconds.
    Seconds(u64),
    /// Suffixed text.
    Text(String),
}

impl DurationSpec {
    /// Resolve to whole seconds.
    pub fn seconds(&self) -> Result<u64> {
        match self {
            DurationSpec::Seconds(n) => Ok(*n),
            DurationSpec::Text(s) => parse_duration(s),
        }
    }
}

/// Parse `15s`, `5m`, `1h` (suffix required for text form).
fn parse_duration(s: &str) -> Result<u64> {
    let s = s.trim();
    let (num, mult) = match s.strip_suffix('s') {
        Some(n) => (n, 1),
        None => match s.strip_suffix('m') {
            Some(n) => (n, 60),
            None => match s.strip_suffix('h') {
                Some(n) => (n, 3600),
                None => {
                    return Err(Error::BadRequest(format!(
                        "invalid duration '{s}': use seconds (15), 15s, 5m or 1h"
                    )));
                }
            },
        },
    };
    num.trim().parse::<u64>().map(|n| n * mult).map_err(|_| {
        Error::BadRequest(format!(
            "invalid duration '{s}': use seconds (15), 15s, 5m or 1h"
        ))
    })
}

/// Health-check block; all fields optional, API defaults apply.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthManifest {
    /// Request path (default `/health`).
    pub path: Option<String>,
    /// Probe interval (default 15s).
    pub interval: Option<DurationSpec>,
    /// Probe timeout (default 5s).
    pub timeout: Option<DurationSpec>,
    /// Consecutive successes to count as healthy (default 2).
    pub healthy_threshold: Option<u64>,
    /// Consecutive failures to count as unhealthy (default 3).
    pub unhealthy_threshold: Option<u64>,
}

/// Resource ceilings (systemd only).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourcesManifest {
    /// Resident memory ceiling in MiB (16–65536).
    pub memory_mb: Option<i64>,
    /// CPU ceiling in percent of one core (1–6400).
    pub cpu_percent: Option<i64>,
}

/// A secret reference: a bare key name, or a key with platform generation.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum SecretRef {
    /// Key name; the value must already be stored.
    Name(String),
    /// Key with options.
    Spec {
        /// Secret key.
        key: String,
        /// Mint a random value at first apply when unset.
        #[serde(default)]
        generate: bool,
    },
}

impl SecretRef {
    /// The secret key.
    pub fn key(&self) -> &str {
        match self {
            SecretRef::Name(k) => k,
            SecretRef::Spec { key, .. } => key,
        }
    }

    /// Whether to mint a random value when the key has none stored.
    pub fn wants_generate(&self) -> bool {
        matches!(self, SecretRef::Spec { generate: true, .. })
    }
}

/// A `turaes.yaml` application manifest.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppManifest {
    /// Lowercase slug (unit + install name).
    pub name: String,
    /// Free text.
    pub description: Option<String>,
    /// Absolute path to the prebuilt binary (service/worker, no `command`).
    pub binary: Option<String>,
    /// Exec argv for interpreted apps (service/worker, no `binary`).
    pub command: Option<Vec<String>>,
    /// Working directory override.
    pub workdir: Option<String>,
    /// Loopback port (service/static; workers take none).
    pub port: Option<u16>,
    /// Primary hostname (service/static).
    pub domain: Option<String>,
    /// Domain aliases (service/static).
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Application kind (default `service`).
    #[serde(default)]
    pub kind: AppKind,
    /// `systemd` (default) or `proc`.
    pub runtime: Option<String>,
    /// Node placement (default `local`).
    pub server: Option<String>,
    /// Restart on unhealthy (default true).
    pub auto_restart: Option<bool>,
    /// Health checks (service only).
    pub health: Option<HealthManifest>,
    /// Metrics path (service only, default `/metrics`).
    pub metrics_path: Option<String>,
    /// Resource ceilings (systemd only).
    pub resources: Option<ResourcesManifest>,
    /// Source directory synced for `static` apps.
    pub publish_dir: Option<String>,
    /// Plaintext, committable environment.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Secret key names (values live in the sealed store, never here).
    #[serde(default)]
    pub secrets: Vec<SecretRef>,
}

fn validate_slug(name: &str) -> Result<()> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
        && !name.ends_with('-');
    if valid {
        Ok(())
    } else {
        Err(Error::BadRequest(
            "name must be a lowercase slug of a-z, 0-9 and '-'".into(),
        ))
    }
}

fn validate_env_key(key: &str) -> Result<()> {
    let valid = !key.is_empty()
        && key.len() <= 128
        && key
            .chars()
            .enumerate()
            .all(|(i, c)| c.is_ascii_alphabetic() || c == '_' || (i > 0 && c.is_ascii_digit()));
    if valid {
        Ok(())
    } else {
        Err(Error::BadRequest(format!(
            "env key '{key}' must match [A-Za-z_][A-Za-z0-9_]*"
        )))
    }
}

fn validate_domain(domain: &str) -> Result<()> {
    let d = domain.trim().to_ascii_lowercase();
    let ok = !d.is_empty()
        && d.len() <= 253
        && d.contains('.')
        && d.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
        && !d.starts_with('.')
        && !d.ends_with('.');
    if ok {
        Ok(())
    } else {
        Err(Error::BadRequest(format!(
            "invalid domain '{domain}': use a lowercase FQDN like www.example.com"
        )))
    }
}

impl AppManifest {
    /// Parse YAML text into a manifest. Unknown fields are rejected so typos
    /// fail loudly with a line number.
    pub fn parse(yaml: &str) -> Result<Self> {
        serde_yaml::from_str(yaml).map_err(|e| Error::BadRequest(format!("invalid manifest: {e}")))
    }

    /// Structural validation mirroring the API rules. Live-state checks
    /// (port conflicts, quotas) happen at apply time.
    pub fn validate(&self) -> Result<()> {
        validate_slug(&self.name)?;

        let runtime = self.runtime.as_deref().unwrap_or("systemd");
        if !matches!(runtime, "systemd" | "proc") {
            return Err(Error::BadRequest(
                "runtime must be 'systemd' or 'proc'".into(),
            ));
        }

        match self.kind {
            AppKind::Service | AppKind::Worker => {
                match (&self.binary, &self.command) {
                    (None, None) => {
                        return Err(Error::BadRequest(
                            "service/worker needs exactly one of 'binary' or 'command'".into(),
                        ));
                    }
                    (Some(_), Some(_)) => {
                        return Err(Error::BadRequest(
                            "'binary' and 'command' are mutually exclusive".into(),
                        ));
                    }
                    _ => {}
                }
                if let Some(argv) = &self.command {
                    if argv.is_empty() || argv.iter().any(|a| a.trim().is_empty()) {
                        return Err(Error::BadRequest(
                            "'command' must be a non-empty argv array".into(),
                        ));
                    }
                    if argv[0].contains(' ') {
                        return Err(Error::BadRequest(
                            "command[0] must be a binary path without spaces (no shell)".into(),
                        ));
                    }
                }
                if let Some(binary) = &self.binary {
                    if binary.trim().is_empty() {
                        return Err(Error::BadRequest("'binary' must not be empty".into()));
                    }
                }
                if self.publish_dir.is_some() {
                    return Err(Error::BadRequest(
                        "'publish_dir' is only valid for kind: static".into(),
                    ));
                }
            }
            AppKind::Static => {
                if self
                    .publish_dir
                    .as_ref()
                    .is_none_or(|d| d.trim().is_empty())
                {
                    return Err(Error::BadRequest("kind: static needs 'publish_dir'".into()));
                }
                if self.binary.is_some() || self.command.is_some() {
                    return Err(Error::BadRequest(
                        "kind: static takes 'publish_dir', not 'binary'/'command'".into(),
                    ));
                }
                if self.health.is_some() {
                    return Err(Error::BadRequest(
                        "kind: static is always healthy; drop the 'health' block".into(),
                    ));
                }
            }
        }

        match self.kind {
            AppKind::Worker => {
                if self.port.is_some() {
                    return Err(Error::BadRequest(
                        "kind: worker takes no 'port' (nothing listens)".into(),
                    ));
                }
                if self.domain.is_some() || !self.aliases.is_empty() {
                    return Err(Error::BadRequest(
                        "kind: worker takes no 'domain'/'aliases' (nothing is routed)".into(),
                    ));
                }
                if self.metrics_path.is_some() {
                    return Err(Error::BadRequest(
                        "kind: worker takes no 'metrics_path' (nothing is scraped)".into(),
                    ));
                }
            }
            AppKind::Service | AppKind::Static => {
                if self.port.is_none() {
                    return Err(Error::BadRequest(
                        "kind: service/static needs 'port'".into(),
                    ));
                }
            }
        }

        if let Some(domain) = &self.domain {
            validate_domain(domain)?;
        }
        for alias in &self.aliases {
            validate_domain(alias)?;
        }

        if let Some(health) = &self.health {
            if let Some(interval) = &health.interval {
                interval.seconds()?;
            }
            if let Some(timeout) = &health.timeout {
                timeout.seconds()?;
            }
        }

        if let Some(resources) = &self.resources {
            if let Some(m) = resources.memory_mb {
                if !(16..=65536).contains(&m) {
                    return Err(Error::BadRequest(
                        "resources.memory_mb must be between 16 and 65536".into(),
                    ));
                }
            }
            if let Some(c) = resources.cpu_percent {
                if !(1..=6400).contains(&c) {
                    return Err(Error::BadRequest(
                        "resources.cpu_percent must be between 1 and 6400".into(),
                    ));
                }
            }
            if runtime == "proc"
                && (resources.memory_mb.is_some() || resources.cpu_percent.is_some())
            {
                return Err(Error::BadRequest(
                    "resource limits require the systemd runtime".into(),
                ));
            }
        }

        for key in self.env.keys() {
            validate_env_key(key)?;
        }
        for secret in &self.secrets {
            let key = secret.key();
            if key.trim().is_empty() {
                return Err(Error::BadRequest("secret entries must name a key".into()));
            }
            validate_env_key(key)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_yaml() -> &'static str {
        r#"
name: beruang
description: Calculator gateway
binary: /srv/beruang/target/release/beruang-gateway
port: 8000
domain: beruang.example.com
aliases: [www.example.com]
runtime: systemd
server: local
auto_restart: true
health:
  path: /health
  interval: 15s
  timeout: 5s
  healthy_threshold: 2
  unhealthy_threshold: 3
metrics_path: /metrics
resources:
  memory_mb: 512
  cpu_percent: 150
env:
  LOG_LEVEL: info
secrets:
  - DATABASE_URL
  - key: INTERNAL_TOKEN
    generate: true
"#
    }

    #[test]
    fn parses_and_validates_full_manifest() {
        let m = AppManifest::parse(full_yaml()).unwrap();
        m.validate().unwrap();
        assert_eq!(m.kind, AppKind::Service);
        assert_eq!(m.aliases, vec!["www.example.com".to_string()]);
        assert_eq!(
            m.health
                .as_ref()
                .unwrap()
                .interval
                .as_ref()
                .unwrap()
                .seconds()
                .unwrap(),
            15
        );
        assert_eq!(m.secrets.len(), 2);
        assert_eq!(m.secrets[0].key(), "DATABASE_URL");
        assert!(!m.secrets[0].wants_generate());
        assert!(m.secrets[1].wants_generate());
    }

    #[test]
    fn rejects_unknown_fields() {
        let err = AppManifest::parse("name: x\nbinary: /b\nportun: 1\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("portun"), "typo should be named: {err}");
    }

    #[test]
    fn durations_accept_seconds_and_suffixes() {
        assert_eq!(DurationSpec::Seconds(90).seconds().unwrap(), 90);
        assert_eq!(DurationSpec::Text("15s".into()).seconds().unwrap(), 15);
        assert_eq!(DurationSpec::Text("5m".into()).seconds().unwrap(), 300);
        assert_eq!(DurationSpec::Text("1h".into()).seconds().unwrap(), 3600);
        assert!(DurationSpec::Text("soon".into()).seconds().is_err());
    }

    #[test]
    fn kind_rules_are_strict() {
        // Worker takes no port/domain.
        let mut m = AppManifest::parse("name: w\ncommand: [/bin/true]\nport: 1\n").unwrap();
        m.kind = AppKind::Worker;
        assert!(m.validate().is_err());

        // Static needs publish_dir and nothing else executable.
        let m =
            AppManifest::parse("name: s\nkind: static\nport: 1\npublish_dir: ./dist\n").unwrap();
        m.validate().unwrap();
        let m = AppManifest::parse("name: s\nkind: static\nport: 1\n").unwrap();
        assert!(m.validate().is_err());
        let m =
            AppManifest::parse("name: s\nkind: static\nport: 1\npublish_dir: ./d\nbinary: /b\n")
                .unwrap();
        assert!(m.validate().is_err());

        // Service needs an executable source, exactly one of binary/command.
        assert!(AppManifest::parse("name: s\nport: 1\n")
            .unwrap()
            .validate()
            .is_err());
        let m = AppManifest::parse("name: s\nport: 1\nbinary: /b\ncommand: [/b]\n").unwrap();
        assert!(m.validate().is_err());
    }

    #[test]
    fn env_and_secret_keys_share_api_rules() {
        let m = AppManifest::parse("name: s\nbinary: /b\nport: 1\nenv:\n  1BAD: x\n").unwrap();
        assert!(m.validate().is_err());
    }
}
